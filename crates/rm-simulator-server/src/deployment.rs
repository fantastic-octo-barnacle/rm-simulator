// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Shared staging placement boundary. This is the simulator's approved base
//! ring, not a rulebook starting-zone assertion. The outer footprint follows
//! the base area registered from Figure 5-24 in world `zones.rs`; the inner
//! platform was traced from the default field-package minimap.
use rm_simulator_world::Team;

/// Base centre in world FLU metres; blue is its point reflection.
pub const RED_BASE_M: [f64; 2] = [11.68, 0.0];
/// Outer red deployment boundary, in world FLU metres.
pub const RED_OUTER_M: [[f64; 2]; 6] = [
    [12.01, -1.93],
    [9.96, -0.77],
    [9.94, 0.84],
    [11.91, 1.96],
    [13.36, 1.17],
    [13.38, -1.14],
];
/// Excluded red base platform, in world FLU metres.
pub const RED_INNER_M: [[f64; 2]; 6] = [
    [11.68, -0.95],
    [10.86, -0.475],
    [10.86, 0.475],
    [11.68, 0.95],
    [12.50, 0.475],
    [12.50, -0.475],
];
/// Mirror a red-side FLU point to the selected team.
///
/// ```
/// use rm_simulator_server::deployment::for_team;
/// use rm_simulator_world::Team;
/// assert_eq!(for_team(Team::Blue, [12.0, -1.0]), [-12.0, 1.0]);
/// ```
pub fn for_team(team: Team, point_m: [f64; 2]) -> [f64; 2] {
    let sign = if team == Team::Red { 1.0 } else { -1.0 };
    point_m.map(|v| sign * v)
}
/// Whether a finite chassis centre lies inside its team's ring, outside the
/// platform. The host additionally checks robot compatibility and occupancy.
///
/// ```
/// use rm_simulator_server::deployment::contains;
/// use rm_simulator_world::Team;
/// assert!(contains(Team::Red, [10.4, 0.0]));
/// assert!(!contains(Team::Red, [11.68, 0.0]));
/// assert!(contains(Team::Blue, [-10.4, 0.0]));
/// ```
pub fn contains(team: Team, point_m: [f64; 2]) -> bool {
    let p = for_team(team, point_m);
    p.into_iter().all(f64::is_finite) && inside(p, &RED_OUTER_M) && !inside(p, &RED_INNER_M)
}
fn inside(p: [f64; 2], polygon: &[[f64; 2]]) -> bool {
    let mut hit = false;
    for i in 0..polygon.len() {
        let [ax, ay] = polygon[i];
        let [bx, by] = polygon[(i + 1) % polygon.len()];
        if (ay > p[1]) != (by > p[1]) && p[0] < (bx - ax) * (p[1] - ay) / (by - ay) + ax {
            hit = !hit;
        }
    }
    hit
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ring_is_mirrored_and_excludes_the_platform_and_nonfinite_points() {
        for team in [Team::Red, Team::Blue] {
            assert!(contains(team, for_team(team, [10.4, 0.0])));
            for p in [
                [11.68, 0.0],
                [8.0, 0.0],
                [-10.4, 0.0],
                [f64::NAN, 0.0],
                [f64::INFINITY, 0.0],
            ] {
                assert!(!contains(team, for_team(team, p)));
            }
        }
    }
}
