// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Provisional shots with bounded retries and independent authoritative outcomes.
use super::*;
use crate::projectile_prediction::{Flight, Request, Worker};
use rm_simulator_server::protocol::ShotResult;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
struct LocalFlight {
    flight: Flight,
    spread: rm_simulator_server::protocol::BulletSpread,
    command: Command,
    created: Instant,
    reserved: bool,
    scheduled: bool,
    muzzle_resolved: bool,
    sent: Instant,
    projectile: Option<rm_simulator_world::ProjectileSnapshot>,
}
/// Provisional shots this client fired and how the host answered them. A
/// provisional ball is drawn under a client-owned id, never the host's.
#[derive(Default)]
pub(super) struct LocalShots {
    /// Wall time from sending a shot to the host accepting it, in ms.
    pub last_confirmation_ms: Option<f64>,
    /// Individual confirmation delays, retained independently of console polling.
    pub confirmation_samples: rm_simulator_server::network_trace::EventSamples,
    /// Individual available host execution offsets, in ms.
    pub execution_samples: rm_simulator_server::network_trace::EventSamples,
    /// Own launches confirmed exactly once, including snapshot recovery.
    pub confirmed_launches: u64,
    /// Host execution time minus the client's predicted launch time, in ms.
    pub last_execution_offset_ms: Option<f64>,
    /// Worker that flies unconfirmed shots through a restored field, or `None`
    /// when prediction is off.
    pub worker: Option<Worker>,
    flights: BTreeMap<u64, LocalFlight>,
    next_launch_ns: u64,
    last_launch: Option<(u64, u64, f64)>,
    /// Shots that expired after 5 s with no authoritative outcome.
    pub unresolved_outcomes: u64,
}
impl Session {
    /// Actual initial speed of this pilot's latest confirmed launch in m/s.
    /// Rejected shots and changes to settings do not change this reading.
    pub fn last_bullet_speed_m_s(&self) -> Option<f64> {
        self.shots.last_launch.map(|(_, _, speed)| speed)
    }
    /// Send a `FireAimed` for the own chassis from the newest input frame and
    /// record a provisional flight. A shot inside the weapon cadence is
    /// skipped; missing input, a full pending set or an exhausted id is an
    /// error the caller shows as a notice.
    pub(super) fn begin_local_shot(&mut self, shooter: u32) -> Result<(), String> {
        if self.weapon_update_pending {
            if !self.commands_confirmed() {
                return Ok(());
            }
            self.weapon_update_pending = false;
        }
        if self.prediction_limited() {
            return Ok(());
        }
        if let Some((chassis, command)) = self.sampled_drive {
            if chassis != shooter {
                return Err("pilot input belongs to another chassis".into());
            }
            if self.last_input_frame.is_none_or(|frame| {
                frame.command != command || frame.sampled_time_ns != self.input_time_ns()
            }) {
                self.apply(Command::Chassis { chassis, command });
            }
        }
        let input = self.last_input_frame.ok_or("waiting for pilot input")?;
        let own = self.presented_chassis().ok_or("pilot unavailable")?;
        if self.pending_shots() >= 32 || self.shots.flights.len() >= 128 {
            return Err("too many pending shots".into());
        }
        if input.sampled_time_ns < self.shots.next_launch_ns {
            return Ok(());
        }
        let muzzle = crate::projectile_prediction::muzzle(own, input.command);
        let muzzle = self
            .weapon
            .spread
            .apply(muzzle, shooter, input.sampled_time_ns);
        let shot_id = self.next_shot_id;
        self.next_shot_id = self
            .next_shot_id
            .checked_add(1)
            .ok_or("shot id exhausted")?;
        let command = Command::FireAimed {
            shooter,
            shot_id,
            input,
        };
        self.client.send(command).map_err(|e| e.to_string())?;
        self.shots.next_launch_ns = input
            .sampled_time_ns
            .saturating_add(self.weapon.interval_ns);
        let now = self.time.now();
        self.shots.flights.insert(
            shot_id,
            LocalFlight {
                spread: self.weapon.spread,
                flight: Flight {
                    id: shot_id,
                    muzzle,
                    launched_ns: input.sampled_time_ns,
                    shot: self.weapon.sample_shot(
                        shooter,
                        input.sampled_time_ns,
                        self.weapon_limits.max_speed_m_s,
                    ),
                    authoritative: None,
                },
                command,
                created: now,
                reserved: true,
                scheduled: false,
                muzzle_resolved: false,
                sent: now,
                projectile: None,
            },
        );
        Ok(())
    }
    /// Unconfirmed shots whose muzzle the prediction worker has not captured
    /// yet, one sample per shot.
    pub(super) fn pending_shot_samples(&self) -> Vec<crate::prediction::ShotSample> {
        self.shots
            .flights
            .iter()
            .filter_map(|(id, flight)| {
                if flight.muzzle_resolved || flight.flight.authoritative.is_some() {
                    return None;
                }
                let Command::FireAimed { input, .. } = flight.command else {
                    return None;
                };
                Some(crate::prediction::ShotSample {
                    id: *id,
                    time_ns: input.sampled_time_ns,
                    aim: input.command,
                })
            })
            .collect()
    }
    /// Apply muzzle poses the prediction worker captured, dropping captures
    /// from an older prediction epoch.
    pub(super) fn resolve_shot_muzzles(
        &mut self,
        captures: Vec<(u64, u64, rm_simulator_world::Pose)>,
    ) {
        for (epoch, id, muzzle) in captures {
            if epoch == self.prediction.epoch
                && let Some(flight) = self.shots.flights.get_mut(&id)
            {
                if let Command::FireAimed { shooter, input, .. } = flight.command {
                    flight.flight.muzzle =
                        flight.spread.apply(muzzle, shooter, input.sampled_time_ns);
                }
                flight.muzzle_resolved = true;
            }
        }
    }
    /// Forget every provisional flight and reopen the cadence gate. Called on
    /// pause, disconnect and any life change.
    pub(super) fn cancel_local_shots(&mut self) {
        self.shots.flights.clear();
        self.shots.next_launch_ns = 0;
    }
    fn shot_result(&mut self, result: ShotResult) {
        if Some(result.shooter) != self.chassis_id {
            return;
        }
        if let (Ok(projectile), Some(executed), Some(speed)) = (
            &result.result,
            result.executed_time_ns,
            result.launch_speed_m_s,
        ) && self
            .shots
            .last_launch
            .is_none_or(|(time, id, _)| (executed, *projectile) > (time, id))
        {
            self.shots.last_launch = Some((executed, *projectile, speed));
        }
        match result.result {
            Ok(id) => {
                if let Some(flight) = self.shots.flights.get_mut(&result.shot_id) {
                    if flight.flight.authoritative.is_none() {
                        self.shots.confirmed_launches += 1;
                        self.shots.last_confirmation_ms =
                            Some(self.time.since(flight.created).as_secs_f64() * 1000.);
                        self.shots.last_execution_offset_ms = result
                            .executed_time_ns
                            .map(|t| (t as f64 - flight.flight.launched_ns as f64) / 1e6);
                        self.shots
                            .confirmation_samples
                            .record(self.shots.last_confirmation_ms.unwrap());
                        if let Some(offset) = self.shots.last_execution_offset_ms {
                            self.shots.execution_samples.record(offset);
                        }
                    }
                    flight.flight.authoritative = Some(id);
                    if let Some(executed) = result.executed_time_ns {
                        flight.flight.launched_ns = executed;
                        self.shots.next_launch_ns = self
                            .shots
                            .next_launch_ns
                            .max(executed.saturating_add(self.weapon.interval_ns));
                    }
                }
            }
            Err(reason) => {
                if self.shots.flights.remove(&result.shot_id).is_some() {
                    self.shot_rejection_count += 1;
                    self.last_shot_rejection = Some(format!("shot {}: {reason}", result.shot_id));
                }
            }
        }
    }
    /// Fold a checkpoint's shot results into the flights and stop reserving
    /// ammo for accepted shots. A confirmed flight is kept only while its
    /// projectile is still in the snapshot.
    pub(super) fn reconcile_shots(
        &mut self,
        state: &rm_simulator_server::simulation::SimulationState,
    ) {
        for result in &state.shot_results {
            self.shot_result(result.clone());
            if Some(result.shooter) == self.chassis_id
                && result.result.is_ok()
                && let Some(flight) = self.shots.flights.get_mut(&result.shot_id)
            {
                flight.reserved = false;
            }
        }
        self.shots.flights.retain(|id, flight| {
            if let Some(authoritative) = flight.flight.authoritative {
                // A snapshot containing acceptance also tells us whether the ball still exists.
                if state.shot_results.iter().any(|r| {
                    Some(r.shooter) == self.chassis_id
                        && r.shot_id == *id
                        && r.result == Ok(authoritative)
                }) {
                    return state
                        .field
                        .projectiles
                        .iter()
                        .any(|p| p.id == authoritative);
                }
            }
            true
        });
    }
    /// Take scheduled and answered shot messages from the client and update
    /// the flights accordingly.
    pub(super) fn poll_shot_results(&mut self) {
        for (shooter, id) in self.client.take_scheduled_shots() {
            if Some(shooter) == self.chassis_id
                && let Some(flight) = self.shots.flights.get_mut(&id)
            {
                flight.scheduled = true;
            }
        }
        for result in self.client.take_shot_results() {
            self.shot_result(result);
        }
    }
    /// Retire flights older than 5 s, resend unconfirmed shots every 40 ms for
    /// at most 250 ms, then step the provisional projectile worker.
    pub(crate) fn advance_local_shots(&mut self) {
        let connected = self.client.disconnected().is_none();
        let now = self.time.now();
        self.shots.flights.retain(|_, flight| {
            let expired = now.saturating_duration_since(flight.created) >= Duration::from_secs(5);
            if expired && flight.flight.authoritative.is_none() {
                self.shots.unresolved_outcomes += 1;
            }
            !expired
        });
        if connected {
            for flight in self.shots.flights.values_mut() {
                if flight.flight.authoritative.is_none()
                    && !flight.scheduled
                    && now.saturating_duration_since(flight.created) < Duration::from_millis(250)
                    && now.saturating_duration_since(flight.sent) >= Duration::from_millis(40)
                {
                    let _ = self.client.send(flight.command);
                    flight.sent = now;
                }
            }
        }
        if self.paused || self.prediction_limited() {
            return;
        }
        let Some(own) = self.presented_chassis().cloned() else {
            return;
        };
        let Some(worker) = &self.shots.worker else {
            return;
        };
        let mut snapshot = self.snapshot.clone();
        for chassis in &mut snapshot.chassis {
            if Some(chassis.id) != self.chassis_id
                && let Some(pose) = self
                    .remote_history
                    .sample(chassis.id, self.remote_view_time_ns)
            {
                *chassis = pose;
            }
        }
        if let Some(output) = worker.exchange(Request {
            epoch: self.prediction.epoch,
            snapshot_id: self.snapshot_id,
            snapshot,
            own,
            time_ns: self.prediction.time_ns.max(self.frame_time_ns),
            flights: self
                .shots
                .flights
                .values()
                .map(|f| f.flight.clone())
                .collect(),
        }) && output.epoch == self.prediction.epoch
        {
            for (id, flight) in &mut self.shots.flights {
                flight.projectile = output.projectiles.get(id).cloned();
            }
        }
    }
    /// Shots of `caliber` begun locally but not yet accepted by the host. The
    /// HUD subtracts them from the host's ammo count.
    pub fn reserved_ammo(&self, caliber: rm_simulator_world::Caliber) -> u32 {
        self.shots
            .flights
            .values()
            .filter(|f| f.reserved && f.flight.shot.caliber == caliber)
            .count() as u32
    }
    /// Flights with no authoritative outcome yet.
    pub fn pending_shots(&self) -> usize {
        self.shots
            .flights
            .values()
            .filter(|f| f.flight.authoritative.is_none())
            .count()
    }
    /// The snapshot plus provisional balls, for drawing. An unconfirmed ball
    /// takes a client-owned id and a confirmed one is replaced by the
    /// predicted state; the snapshot is borrowed unchanged when no flight is
    /// pending.
    pub fn visual_snapshot(&self) -> std::borrow::Cow<'_, FieldSnapshot> {
        if self.shots.flights.is_empty() {
            return std::borrow::Cow::Borrowed(&self.snapshot);
        }
        let mut snapshot = self.snapshot.clone();
        for flight in self.shots.flights.values() {
            if let Some(id) = flight.flight.authoritative
                && let Some(state) = snapshot.projectiles.iter_mut().find(|p| p.id == id)
            {
                state.id = crate::projectile_prediction::PROVISIONAL_BIT | flight.flight.id;
                if let Some(predicted) = &flight.projectile {
                    *state = predicted.clone();
                }
                continue;
            }
            if let Some(projectile) = &flight.projectile {
                snapshot.projectiles.push(projectile.clone());
            }
        }
        std::borrow::Cow::Owned(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weapon_update_waits_for_confirmation_and_old_predictions_keep_their_spread() {
        let mut session = crate::session::test_session(false);
        let shooter = session.chassis_id.unwrap();
        session.number_input(Command::Chassis {
            chassis: shooter,
            command: Default::default(),
        });
        session.begin_local_shot(shooter).unwrap();
        let id = *session.shots.flights.first_key_value().unwrap().0;
        let mut weapon = session.weapon;
        weapon.spread.angle_rad = 0.1;
        session.set_weapon(weapon).unwrap();
        assert!(session.weapon_update_pending);
        assert!(!session.commands_confirmed());
        session.begin_local_shot(shooter).unwrap();
        assert_eq!(session.shots.flights.len(), 1);
        let muzzle = rm_simulator_world::Pose::at([1., 2., 3.]);
        session.resolve_shot_muzzles(vec![(session.prediction.epoch, id, muzzle)]);
        let expected = session.shots.flights[&id].spread.apply(
            muzzle,
            shooter,
            session.last_input_frame.unwrap().sampled_time_ns,
        );
        assert_eq!(session.shots.flights[&id].flight.muzzle, expected);
        assert_eq!(session.weapon, weapon);
    }

    #[test]
    fn execution_binds_actual_tick_and_snapshot_releases_ammo_once() {
        let mut session = crate::session::test_session(false);
        let shooter = session.chassis_id.unwrap();
        session.number_input(Command::Chassis {
            chassis: shooter,
            command: Default::default(),
        });
        session.begin_local_shot(shooter).unwrap();
        let (&shot_id, flight) = session.shots.flights.first_key_value().unwrap();
        let executed = flight.flight.launched_ns + 10_000_000;
        let result = ShotResult {
            launch_speed_m_s: Some(24.87),
            executed_time_ns: Some(executed),
            shooter,
            shot_id,
            result: Ok(999),
        };
        session.shot_result(result.clone());
        session.shot_result(result.clone());
        assert_eq!(
            session.shots.flights[&shot_id].flight.authoritative,
            Some(999)
        );
        assert_eq!(session.shots.flights[&shot_id].flight.launched_ns, executed);
        assert_eq!(session.reserved_ammo(session.weapon.shot.caliber), 1);
        assert_eq!(session.shots.last_execution_offset_ms, Some(10.));
        assert_eq!(session.last_bullet_speed_m_s(), Some(24.87));
        let mut older = result.clone();
        older.executed_time_ns = Some(executed - 1);
        older.launch_speed_m_s = Some(20.);
        session.shot_result(older);
        assert_eq!(session.last_bullet_speed_m_s(), Some(24.87));
        let state = rm_simulator_server::simulation::SimulationState {
            bots: vec![],
            snapshot_id: session.snapshot_id,
            input_epoch: session.input_epoch,
            shot_results: vec![result],
            paused: false,
            field: session.snapshot.clone(),
        };
        session.reconcile_shots(&state);
        session.reconcile_shots(&state);
        let confirmations = serde_json::to_value(&session.shots.confirmation_samples).unwrap();
        let executions = serde_json::to_value(&session.shots.execution_samples).unwrap();
        assert_eq!(confirmations["total"], 1);
        assert_eq!(executions["values"], serde_json::json!([[1, 10.]]));
        assert_eq!(session.reserved_ammo(session.weapon.shot.caliber), 0);
        assert_eq!(session.pending_shots(), 0);
    }

    #[test]
    fn accepted_bullets_do_not_fill_the_pending_shot_limit() {
        let mut session = crate::session::test_session(false);
        let shooter = session.chassis_id.unwrap();
        session.number_input(Command::Chassis {
            chassis: shooter,
            command: Default::default(),
        });
        let input = session.last_input_frame.unwrap();
        let command = Command::FireAimed {
            shooter,
            shot_id: 1,
            input,
        };
        for id in 1..=32 {
            session.shots.flights.insert(
                id,
                LocalFlight {
                    spread: Default::default(),
                    flight: Flight {
                        id,
                        muzzle: rm_simulator_world::Pose::at([0.; 3]),
                        launched_ns: 0,
                        shot: session.weapon.shot,
                        authoritative: Some(id),
                    },
                    command,
                    created: Instant::now(),
                    sent: Instant::now(),
                    reserved: false,
                    scheduled: false,
                    muzzle_resolved: false,
                    projectile: None,
                },
            );
        }
        session.next_shot_id = 33;
        session.begin_local_shot(shooter).unwrap();
        assert_eq!(session.shots.flights.len(), 33);
        assert_eq!(session.pending_shots(), 1);
        for flight in session.shots.flights.values_mut() {
            flight.flight.authoritative = None;
        }
        assert_eq!(
            session.begin_local_shot(shooter).unwrap_err(),
            "too many pending shots"
        );
    }
}
