// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! A deterministic end-to-end network trace: the real host, the real client
//! session and the real UDP codec, with a scripted link in place of a socket.
//!
//! Nothing here sleeps, opens a socket, reads the wall clock or loads CAD. One
//! [`ManualTime`] paces the host worker and the session together; each step
//! delivers the datagrams that have arrived, runs one client frame on the
//! frame grid, advances the tick and pumps what the host produced back into the
//! link. Because every deadline is that one clock, a seed replays byte for byte
//! and an impairment profile is a property of the scenario, not of the machine.
//!
//! What the host worker does between two of our requests is fenced explicitly:
//! client datagrams are submitted and drained *before* time moves, so a tick
//! never splits one step's inputs. Prediction requests are also fenced before
//! the trace advances time, preserving their one-frame result delay without
//! making comparison coverage depend on OS thread scheduling.
use crate::args::Args;
use crate::session::Session;
use rm_simulator_server::clock::{ManualTime, TimeSource};
use rm_simulator_server::host::{Host, HostHandle};
use rm_simulator_server::layout::LayoutOptions;
use rm_simulator_server::net::{Client, ClientLeg};
use rm_simulator_server::protocol::{Command, Role};
use rm_simulator_server::scripted_link::{Impairment, Link};
use rm_simulator_server::simulation::Simulation;
use rm_simulator_server::udp_codec::HostPeer;
use rm_simulator_world::{ChassisCommand, ChassisConfig, Team};
use sha2::{Digest, Sha256};
use std::time::Duration;

/// The host tick period; also the step of this trace.
const STEP_MS: u64 = 2;
/// One client frame every four steps: a steady 125 fps.
const FRAME_STEPS: u64 = 4;
/// Pacing budgets, fixed here so no environment variable can change a trace.
const UP_BYTES_S: u32 = 64 * 1024;
const DOWN_BYTES_S: u32 = 256 * 1024;

/// One scripted run. Field order is the drop order: the session and its client
/// go before the leg, the peer releases its seat before the host stops.
struct Trace {
    session: Session,
    leg: ClientLeg,
    link: Link,
    peer: HostPeer,
    handle: HostHandle,
    /// Kept only to own the worker; it stops when the trace drops.
    #[allow(dead_code)]
    host: Host,
    time: ManualTime,
    clock: TimeSource,
    step: u64,
    chassis: u32,
    /// Everything the client observed, hashed in order.
    digest: Sha256,
    view_time_ns: u64,
    max_correction_m: f64,
    /// Exact-tick comparisons the prediction worker completed.
    comparisons: u64,
    max_pinned_baselines: usize,
    replayed_finalized_inputs: u64,
    last_input_sequence: u64,
    out_of_order_inputs: u64,
    view_regressions: u64,
    recovered_at_step: Option<u64>,
}

/// Build the field the way `--no-field-collision --no-rune --no-referee` does,
/// so no CAD file is read and the chassis stands on the implicit floor.
fn harness_args(predict: bool) -> Args {
    use clap::Parser;
    let mut args = Args::try_parse_from([
        "rm-simulator",
        "--no-field-collision",
        "--no-rune",
        "--no-referee",
    ])
    .expect("harness arguments");
    args.no_prediction = !predict;
    args
}

impl Trace {
    fn open(seed: u64, predict: bool, upstream: Impairment, downstream: Impairment) -> Self {
        let time = ManualTime::new();
        let clock = time.source();
        let args = harness_args(predict);
        let cad = crate::session::test_cad_assets();
        let simulation = Simulation::from_cad(
            &cad,
            &LayoutOptions {
                rune: None,
                outpost_speed_rad_s: 0.0,
                terrain: false,
                referee: false,
                projectile_policy: Default::default(),
            },
            Some(ChassisConfig::default()),
            false,
            |_| {},
        )
        .expect("field from the stand-in package");
        let host = Host::with_time(simulation, true, clock.clone()).expect("host worker");
        let handle = host.handle();
        handle.start_clock().expect("host clock");
        let mut peer = HostPeer::new(handle.clone(), clock.now(), DOWN_BYTES_S, false);
        let mut leg = ClientLeg::new(
            "scripted pilot",
            Some(Team::Red),
            Role::Pilot,
            clock.clone(),
            UP_BYTES_S,
        )
        .expect("client leg");
        let mut link = Link::new(clock.now(), seed, upstream, downstream);
        // The handshake runs on the same scripted link as everything else.
        let mut handshaken = false;
        for _ in 0..2_000 {
            exchange(
                &clock,
                &time,
                &handle,
                &mut peer,
                &mut leg,
                &mut link,
                || {},
            );
            if leg.welcome().is_some() && leg.snapshot_ready() {
                handshaken = true;
                break;
            }
        }
        assert!(handshaken, "the host never welcomed the scripted client");
        let client = Client::over_link(&mut leg, clock.clone()).expect("client over the link");
        let opened = Session::from_client(client, clock.clone(), &args, &cad, [0.; 3], |_, _| {})
            .expect("session over the link");
        let mut session = opened.session;
        session.synchronize_prediction_for_test();
        let chassis = session.chassis_id.expect("the pilot was given a chassis");
        assert_eq!(predict, session.predicts());
        Self {
            view_time_ns: session.remote_view_time_ns,
            session,
            leg,
            link,
            peer,
            handle,
            host,
            time,
            clock,
            step: 0,
            chassis,
            digest: Sha256::new(),
            max_correction_m: 0.,
            comparisons: 0,
            max_pinned_baselines: 0,
            replayed_finalized_inputs: 0,
            last_input_sequence: 0,
            out_of_order_inputs: 0,
            view_regressions: 0,
            recovered_at_step: None,
        }
    }

    /// Advance one 2 ms step, running a client frame on the frame grid.
    fn step(&mut self, mut frame: impl FnMut(&mut Session, u64)) {
        let step = self.step;
        let frame_due = step.is_multiple_of(FRAME_STEPS);
        exchange(
            &self.clock,
            &self.time,
            &self.handle,
            &mut self.peer,
            &mut self.leg,
            &mut self.link,
            || {
                if frame_due {
                    frame(&mut self.session, step);
                }
            },
        );
        self.step += 1;
        if frame_due {
            self.observe();
        }
    }

    /// Record this frame's invariants and fold it into the run digest.
    fn observe(&mut self) {
        let session = &self.session;
        self.max_pinned_baselines = self.max_pinned_baselines.max(self.leg.pinned_baselines());
        if session.remote_view_time_ns < self.view_time_ns {
            self.view_regressions += 1;
        }
        self.view_time_ns = session.remote_view_time_ns;
        // A checkpoint at T finalizes every interval ending at or before T. An
        // input older than that must never reach another replay.
        if let Some(pending) = session.pending_inputs() {
            self.replayed_finalized_inputs += pending
                .iter()
                .filter(|input| input.time_ns < session.snapshot.time_ns)
                .count() as u64;
            // The queue is a suffix of the client's own numbering: strictly
            // increasing inside one replay, and never rewound between frames.
            if pending.windows(2).any(|w| w[1].sequence <= w[0].sequence) {
                self.out_of_order_inputs += 1;
            }
            if let Some(newest) = pending.last() {
                if newest.sequence < self.last_input_sequence {
                    self.out_of_order_inputs += 1;
                }
                self.last_input_sequence = newest.sequence;
            }
        }
        if let Some(correction) = session.correction_stats() {
            self.comparisons = correction.comparisons;
            if let Some(position_m) = correction.position_m {
                self.max_correction_m = self.max_correction_m.max(position_m);
            }
        }
        let own = session
            .presented_chassis()
            .expect("the pilot keeps its chassis");
        self.digest.update(session.snapshot.tick.to_le_bytes());
        self.digest.update(session.snapshot.time_ns.to_le_bytes());
        self.digest
            .update(session.remote_view_time_ns.to_le_bytes());
        self.digest
            .update(session.presentation_time_ns().to_le_bytes());
        self.digest.update((session.paused as u8).to_le_bytes());
        self.digest.update(session.shot_intents().to_le_bytes());
        self.digest
            .update((session.pending_shots() as u64).to_le_bytes());
        self.digest.update(own.placement_revision.to_le_bytes());
        for value in own.pose.translation_m.into_iter().chain(own.velocity_m_s) {
            self.digest.update(value.to_bits().to_le_bytes());
        }
    }

    fn digest(&self) -> String {
        format!("{:x}", self.digest.clone().finalize())
    }

    fn simulated(&self) -> Duration {
        Duration::from_millis(self.step * STEP_MS)
    }

    /// Shots the host actually created for this pilot.
    fn host_shots(&self) -> u64 {
        self.handle.snapshot().expect("host state").shots_fired
    }

    /// Every begun shot ended exactly once: as a host shot, a rejection, an
    /// expiry, or still in flight. Nothing is counted twice and nothing is lost.
    fn assert_shot_accounting(&self) {
        let session = &self.session;
        let resolved = self.host_shots()
            + session.shot_rejections()
            + session.unresolved_shot_outcomes()
            + session.pending_shots() as u64;
        assert_eq!(
            session.shot_intents(),
            resolved,
            "shot outcomes must account for every intent exactly once"
        );
    }

    fn assert_common_invariants(&self) {
        assert!(
            self.session.connection_toast != Some("Disconnected"),
            "the scripted link never closes the connection"
        );
        assert_eq!(
            self.replayed_finalized_inputs, 0,
            "a finalized input was offered to another replay"
        );
        assert_eq!(
            self.out_of_order_inputs, 0,
            "an input sequence was reissued"
        );
        assert_eq!(self.view_regressions, 0, "remote view time went backwards");
        assert!(
            self.max_pinned_baselines <= 2,
            "the decoder pinned {} baselines",
            self.max_pinned_baselines
        );
        self.assert_shot_accounting();
    }
}

/// One exchange across the link. `frame` runs after inbound datagrams have been
/// decoded and before outbound ones are encoded, so a frame's commands leave in
/// the same step they were made.
fn exchange(
    clock: &TimeSource,
    time: &ManualTime,
    handle: &HostHandle,
    peer: &mut HostPeer,
    leg: &mut ClientLeg,
    link: &mut Link,
    frame: impl FnOnce(),
) {
    let now = clock.now();
    for payload in link.take_to_client(now) {
        leg.deliver(payload);
    }
    leg.pump().expect("client leg");
    frame();
    leg.pump().expect("client leg");
    for packet in leg.take_outgoing() {
        link.send_to_host(now, packet);
    }
    for payload in link.take_to_host(now) {
        peer.deliver(&payload, now).expect("host peer");
    }
    // Fence before moving time: everything this step delivered is applied to
    // the simulation while the clock still reads the previous tick.
    handle.roster().expect("host fence");
    time.advance_ms(STEP_MS);
    // Two fences bracket exactly one `advance_clock` on the worker: the first
    // reply may precede it, the second cannot.
    handle.roster().expect("host fence");
    handle.roster().expect("host fence");
    let now = clock.now();
    peer.pump(now, 0, 16, 64).expect("host peer");
    for packet in peer.take_outgoing() {
        link.send_to_client(now, packet);
    }
}

/// A fixed pilot script: drive, turn, strafe, coast, sweeping the aim
/// throughout, and fire on a cadence the weapon's interval actually allows.
fn pilot_frame(session: &mut Session, chassis: u32, step: u64, firing: bool) {
    session.poll().expect("session poll");
    let phase = (step / 200) % 4;
    let command = ChassisCommand {
        forward_m_s: if phase == 0 || phase == 1 { 1.5 } else { 0. },
        left_m_s: if phase == 2 { 1.0 } else { 0. },
        yaw_rate_rad_s: if phase == 1 { 0.6 } else { 0. },
        aim_yaw_rad: (step as f64) * 0.0015,
        aim_pitch_rad: 0.1,
    };
    session.drive(chassis, command, step.is_multiple_of(200));
    if firing && step.is_multiple_of(100) {
        session.apply(Command::Fire { shooter: chassis });
    }
    session.advance_local_shots();
}

fn clean() -> Impairment {
    Impairment {
        delay: Duration::from_millis(15),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3.2 s of driving and firing over an unimpaired 30 ms round trip.
    #[test]
    fn delayed_contact_event_and_snapshot_recovery_flash_once() {
        let mut trace = Trace::open(
            2026,
            false,
            Impairment::default(),
            Impairment {
                delay: Duration::from_millis(100),
                ..Default::default()
            },
        );
        let snapshot = trace.handle.snapshot().unwrap();
        let plate = snapshot.outposts[0].armors[0].pose;
        let rotation = crate::frames::dquat(plate.rotation_wxyz);
        let muzzle = rm_simulator_world::Pose {
            translation_m: (bevy::math::DVec3::from_array(plate.translation_m)
                + rotation * bevy::math::DVec3::X * 0.05)
                .to_array(),
            rotation_wxyz: crate::frames::wxyz(
                rotation * bevy::math::DQuat::from_rotation_z(std::f64::consts::PI),
            ),
        };
        trace
            .handle
            .apply(&Command::SpawnProjectile {
                muzzle,
                shot: rm_simulator_world::Shot {
                    caliber: rm_simulator_world::Caliber::Mm17,
                    speed_m_s: 20.,
                },
            })
            .unwrap();
        let mut flashes = 0;
        let mut was_visible = false;
        for _ in 0..400 {
            trace.step(|session, _| session.poll().unwrap());
            let visible = trace
                .session
                .hit_feedback
                .visible(trace.clock.now(), 50_000_000)
                .next()
                .is_some();
            if visible && !was_visible {
                flashes += 1;
            }
            was_visible = visible;
        }
        assert!(trace.handle.snapshot().unwrap().hits_detected > snapshot.hits_detected);
        assert_eq!(
            flashes, 1,
            "reliable event and snapshot recovery share one flash"
        );
        assert!(!was_visible);
    }

    #[test]
    fn a_clean_link_predicts_within_bounds_and_resolves_every_shot() {
        let mut trace = Trace::open(1, true, clean(), clean());
        let chassis = trace.chassis;
        for _ in 0..1_600 {
            trace.step(|session, step| pilot_frame(session, chassis, step, true));
        }
        // Let every shot in flight reach its outcome before accounting.
        for _ in 0..400 {
            trace.step(|session, step| pilot_frame(session, chassis, step, false));
        }
        eprintln!(
            "clean {:?}: {} shots, {} comparisons, worst {:.4} m, {} deltas, {} pinned, link {:?}/{:?}",
            trace.simulated(),
            trace.host_shots(),
            trace.comparisons,
            trace.max_correction_m,
            trace.leg.transport_stats().decoded_deltas,
            trace.max_pinned_baselines,
            trace.link.upstream(),
            trace.link.downstream(),
        );
        trace.assert_common_invariants();
        assert!(trace.host_shots() > 8, "the pilot never got to shoot");
        assert_eq!(trace.session.shot_intents(), trace.host_shots());
        assert_eq!(trace.session.unresolved_shot_outcomes(), 0);
        assert_eq!(trace.session.shot_rejections(), 0);
        assert_eq!(trace.session.pending_shots(), 0);
        assert!(
            trace.comparisons > 100,
            "prediction compared too few exact ticks ({} comparisons)",
            trace.comparisons
        );
        assert!(
            trace.max_correction_m < 0.01,
            "worst clean-link correction {} m",
            trace.max_correction_m
        );
        assert!(
            trace.leg.transport_stats().decoded_deltas > 50,
            "the encoder never settled into delta coding"
        );
    }

    /// The same seed twice is the same trace, byte for byte.
    #[test]
    fn a_seeded_run_replays_exactly() {
        let run = |seed, predict| {
            let upstream = Impairment {
                loss: 0.12,
                duplicate: 0.05,
                reorder_every: 9,
                reorder_hold: 3,
                delay: Duration::from_millis(25),
                jitter: Duration::from_millis(12),
                recovery: Duration::from_millis(60),
                ..Default::default()
            };
            let mut trace = Trace::open(seed, predict, upstream.clone(), upstream);
            let chassis = trace.chassis;
            for _ in 0..900 {
                trace.step(|session, step| pilot_frame(session, chassis, step, true));
            }
            (trace.digest(), trace.link.downstream(), trace.host_shots())
        };
        for predict in [false, true] {
            let first = run(99, predict);
            assert_eq!(
                first,
                run(99, predict),
                "a seeded trace must be reproducible with prediction={predict}"
            );
            let other = run(100, predict);
            assert_ne!(
                first.0, other.0,
                "a different seed must script a different link"
            );
            eprintln!("seeded digest {} over {:?}", first.0, first.1);
        }
    }

    /// Loss, reordering, duplication and a blackout, each on its own trace.
    #[test]
    fn impairment_keeps_the_invariants_and_the_session_recovers() {
        let profiles: [(&str, Impairment, Impairment); 4] = [
            (
                "10% loss",
                Impairment {
                    loss: 0.10,
                    delay: Duration::from_millis(20),
                    recovery: Duration::from_millis(60),
                    ..Default::default()
                },
                Impairment {
                    loss: 0.10,
                    delay: Duration::from_millis(20),
                    recovery: Duration::from_millis(60),
                    ..Default::default()
                },
            ),
            (
                "30% loss",
                Impairment {
                    loss: 0.30,
                    delay: Duration::from_millis(30),
                    jitter: Duration::from_millis(15),
                    recovery: Duration::from_millis(90),
                    ..Default::default()
                },
                Impairment {
                    loss: 0.30,
                    delay: Duration::from_millis(30),
                    jitter: Duration::from_millis(15),
                    recovery: Duration::from_millis(90),
                    ..Default::default()
                },
            ),
            (
                "reorder and duplicate",
                Impairment {
                    duplicate: 0.20,
                    reorder_every: 4,
                    reorder_hold: 5,
                    drop_packets: vec![11, 12, 13],
                    delay: Duration::from_millis(20),
                    recovery: Duration::from_millis(60),
                    ..Default::default()
                },
                Impairment {
                    duplicate: 0.20,
                    reorder_every: 3,
                    reorder_hold: 6,
                    delay: Duration::from_millis(20),
                    recovery: Duration::from_millis(60),
                    ..Default::default()
                },
            ),
            (
                "600 ms blackout",
                Impairment {
                    delay: Duration::from_millis(20),
                    recovery: Duration::from_millis(60),
                    blackout: Some((Duration::from_millis(800), Duration::from_millis(1_400))),
                    ..Default::default()
                },
                Impairment {
                    delay: Duration::from_millis(20),
                    recovery: Duration::from_millis(60),
                    blackout: Some((Duration::from_millis(800), Duration::from_millis(1_400))),
                    ..Default::default()
                },
            ),
        ];
        for (name, upstream, downstream) in profiles {
            let blackout = downstream.blackout;
            let mut trace = Trace::open(2_026, true, upstream, downstream);
            let chassis = trace.chassis;
            let mut deltas_before_blackout = 0;
            for _ in 0..1_200 {
                trace.step(|session, step| pilot_frame(session, chassis, step, true));
                if let Some((_, end)) = blackout {
                    if trace.simulated() < end {
                        deltas_before_blackout = trace.leg.transport_stats().decoded_deltas;
                        trace.recovered_at_step = None;
                    } else if trace.recovered_at_step.is_none()
                        && trace.session.connection_toast.is_none()
                        && trace.leg.transport_stats().decoded_deltas > deltas_before_blackout
                    {
                        trace.recovered_at_step = Some(trace.step);
                    }
                }
            }
            for _ in 0..400 {
                trace.step(|session, step| pilot_frame(session, chassis, step, false));
            }
            eprintln!(
                "{name}: {} shots, {} unresolved, {} rejected, {} deltas, {} pinned, {} comparisons worst {:.4} m, link {:?}/{:?}",
                trace.host_shots(),
                trace.session.unresolved_shot_outcomes(),
                trace.session.shot_rejections(),
                trace.leg.transport_stats().decoded_deltas,
                trace.max_pinned_baselines,
                trace.comparisons,
                trace.max_correction_m,
                trace.link.upstream(),
                trace.link.downstream(),
            );
            trace.assert_common_invariants();
            assert!(
                trace.host_shots() > 0,
                "{name}: no shot ever reached the host"
            );
            assert!(
                trace.leg.transport_stats().decoded_deltas > 20,
                "{name}: the encoder never resumed delta coding ({} deltas)",
                trace.leg.transport_stats().decoded_deltas
            );
            assert!(
                trace.comparisons > 25,
                "{name}: prediction compared only {} exact ticks",
                trace.comparisons
            );
            assert!(
                trace.max_correction_m < 0.5,
                "{name}: worst correction {} m",
                trace.max_correction_m
            );
            assert!(
                trace.session.unresolved_shot_outcomes() <= 2,
                "{name}: {} shots never resolved",
                trace.session.unresolved_shot_outcomes()
            );
            if let Some((_, end)) = blackout {
                let recovered = trace
                    .recovered_at_step
                    .unwrap_or_else(|| panic!("{name}: the session never recovered"));
                let after = Duration::from_millis(recovered * STEP_MS) - end;
                eprintln!("{name}: recovered {after:?} after the blackout");
                assert!(
                    after < Duration::from_millis(500),
                    "{name}: recovery took {after:?}"
                );
            }
        }
    }

    /// A confirmed command's effect is in the checkpoint the host sends before
    /// its `Pong`, so the first confirmed frame already shows it.
    #[test]
    fn a_confirmation_never_overtakes_the_snapshot_that_carries_it() {
        let mut trace = Trace::open(5, false, clean(), clean());
        let chassis = trace.chassis;
        for _ in 0..200 {
            trace.step(|session, step| pilot_frame(session, chassis, step, false));
        }
        let before = trace
            .session
            .own_chassis()
            .expect("own chassis")
            .placement_revision;
        trace
            .session
            .apply_confirmed(Command::ResetRobot { chassis })
            .expect("queued the reset");
        assert!(!trace.session.commands_confirmed());
        let mut confirmed = false;
        for _ in 0..400 {
            trace.step(|session, _| session.poll().expect("session poll"));
            if trace.session.commands_confirmed() {
                assert_eq!(
                    trace.session.rejection_count, 0,
                    "{:?}",
                    trace.session.last_rejection
                );
                assert!(
                    trace
                        .session
                        .own_chassis()
                        .expect("own chassis")
                        .placement_revision
                        > before,
                    "the Pong arrived before the snapshot that confirms it"
                );
                confirmed = true;
                break;
            }
        }
        assert!(confirmed, "the confirmation never arrived");
    }
}
