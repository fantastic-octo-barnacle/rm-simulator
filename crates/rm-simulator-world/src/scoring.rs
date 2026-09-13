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
                let state = &mut self.bases[base as usize];
                // Outpost immunity applies during a match; Idle is training.
                let protected = self
                    .referee
                    .as_ref()
                    .is_some_and(|r| r.base_protected(state.config.team));
                if !protected {
                    let damage = base::damage(contact.caliber, plate, offset);
                    let defense = self
                        .referee
                        .as_ref()
                        .map_or(0, |r| r.base_defense_pct(state.config.team));
                    hit.damage = state.damage(referee::defended(damage, defense));
                    if let Some(referee) = &mut self.referee {
                        referee.observe_base_hit(
                            state.config.team,
                            hit.damage,
                            state.hp,
                            state.shield_hp,
                            contact.shooter,
                        );
                    }
                }
            }
            ArmorTarget::Outpost { outpost, .. } => {
                let mut damage = projectile::outpost_damage(contact.caliber, offset);
                if let Some(referee) = &self.referee {
                    damage =
                        referee::defended(damage, referee.outpost_defense_pct(outpost as usize));
                }
                hit.damage = self.outposts[outpost as usize].damage(time_ns, damage);
            }
            ArmorTarget::Rune { rune, blade } => {
                let outcome: HitOutcome = self.runes[rune as usize].hit(time_ns, blade)?;
                hit.rune_outcome = Some(outcome);
            }
            ArmorTarget::Chassis { chassis, .. } => {
                // Table 5-2 robot damage; the referee keeps the HP, so
                // without one the strike is only reported.
                let damage = contact.caliber.robot_damage();
                hit.damage = match &mut self.referee {
                    Some(referee) => referee.hit_robot(chassis, damage, contact.shooter),
                    None => damage,
                };
            }
        }
        Ok(hit)
    }
}
