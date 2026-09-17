// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Contact detection and gameplay consequences, applied in physical contact order.
use super::*;

/// A contact either passes every detection condition or has one rejection reason.
enum DetectionOutcome {
    Rejected(Rejection),
    Detected { offset_m: [f64; 2] },
}

impl Field {
    fn detect(&self, time_ns: u64, contact: &Contact) -> DetectionOutcome {
        let is_rune = matches!(contact.target, ArmorTarget::Rune { .. });
        let Some(offset) = projectile::scoring_offset(contact.target, contact.local_m) else {
            return DetectionOutcome::Rejected(Rejection::OutsideTarget);
        };
        if is_rune && contact.caliber != Caliber::Mm17 {
            return DetectionOutcome::Rejected(Rejection::Caliber);
        }
        if contact.normal_speed_m_s <= contact.caliber.armor_detection_speed_m_s() {
            return DetectionOutcome::Rejected(Rejection::NormalSpeed);
        }
        if let Some(last) = self.last_detection_ns.get(&contact.target)
            && time_ns.saturating_sub(*last) < contact.caliber.detection_interval_ns()
        {
            return DetectionOutcome::Rejected(Rejection::DetectionInterval);
        }
        if let (ArmorTarget::Rune { rune, .. }, Some(referee)) = (contact.target, &self.referee)
            && referee::ring_of(offset) < referee.rune_min_ring(rune)
        {
            return DetectionOutcome::Rejected(Rejection::DisabledRing);
        }
        DetectionOutcome::Detected { offset_m: offset }
    }
    /// Apply the rule manual's detection conditions to one raw contact.
    pub(super) fn score(&mut self, time_ns: u64, contact: Contact) -> Result<ArmorHit, FieldError> {
        let mut hit = ArmorHit {
            time_ns,
            projectile: contact.projectile,
            shooter: contact.shooter,
            caliber: contact.caliber,
            target: contact.target,
            position_m: contact.position_m,
            local_offset_m: [contact.local_m[1], contact.local_m[2]],
            normal_speed_m_s: contact.normal_speed_m_s,
            detected: false,
            rejection: None,
            rune_outcome: None,
            damage: 0,
        };
        let offset = match self.detect(time_ns, &contact) {
            DetectionOutcome::Rejected(reason) => {
                hit.rejection = Some(reason);
                return Ok(hit);
            }
            DetectionOutcome::Detected { offset_m } => offset_m,
        };
        hit.local_offset_m = offset;
        self.last_detection_ns.insert(contact.target, time_ns);
        hit.detected = true;
        self.hits_detected += 1;
        match contact.target {
            ArmorTarget::Base { base, plate } => {
                let team = self.bases[base as usize].config.team;
                hit.damage = match &mut self.referee {
                    // The gameplay engine applies outpost protection, buffs,
                    // the Table 5-2 values and the section 5.5.1 centre square.
                    // The seventh (dart) plate accepting projectiles is a
                    // training override without the centre bonus.
                    Some(referee) => {
                        let applied =
                            referee.projectile_hit(rm_simulator_gameplay::ProjectileHit {
                                target: rm_simulator_gameplay::Target::Base(referee::game_team(
                                    team,
                                )),
                                caliber: referee::game_caliber(contact.caliber),
                                shooter: contact.shooter,
                                upper_front: plate == 3,
                                critical: plate != 6 && projectile::in_centre_square(offset),
                            });
                        applied.hp + applied.shield
                    }
                    None => self.bases[base as usize].damage(base::damage(
                        contact.caliber,
                        plate,
                        offset,
                    )),
                };
                self.sync_structures(time_ns);
            }
            ArmorTarget::Outpost { outpost, .. } => {
                hit.damage = match &mut self.referee {
                    Some(referee) => {
                        let team = referee.config().outpost_teams[outpost as usize];
                        referee
                            .projectile_hit(rm_simulator_gameplay::ProjectileHit {
                                target: rm_simulator_gameplay::Target::Outpost(referee::game_team(
                                    team,
                                )),
                                caliber: referee::game_caliber(contact.caliber),
                                shooter: contact.shooter,
                                upper_front: false,
                                critical: projectile::in_centre_square(offset),
                            })
                            .hp
                    }
                    None => self.outposts[outpost as usize]
                        .damage(time_ns, projectile::outpost_damage(contact.caliber, offset)),
                };
                self.sync_structures(time_ns);
            }
            ArmorTarget::Rune { rune, blade } => {
                let outcome: HitOutcome = self.runes[rune as usize].hit(time_ns, blade)?;
                hit.rune_outcome = Some(outcome);
            }
            ArmorTarget::Chassis { chassis, .. } => {
                // Table 5-2 robot damage; the gameplay engine keeps the HP, so
                // without a referee the strike is only reported.
                hit.damage = match &mut self.referee {
                    Some(referee) => {
                        referee
                            .projectile_hit(rm_simulator_gameplay::ProjectileHit {
                                target: rm_simulator_gameplay::Target::Robot(chassis),
                                caliber: referee::game_caliber(contact.caliber),
                                shooter: contact.shooter,
                                upper_front: false,
                                critical: false,
                            })
                            .hp
                    }
                    None => contact.caliber.robot_damage(),
                };
            }
        }
        Ok(hit)
    }
}
