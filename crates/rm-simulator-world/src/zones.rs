// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Buff point footprints on the field (section 5.5.3.1, Figure 5-24).
//!
//! A robot occupies a point while its chassis body centre lies over the
//! point's footprint and within [`ZONE_BODY_BAND_M`] above its floor. The
//! rulebook prints no coordinates for the RFID card areas: the footprints
//! below were read off Figure 5-24 of the V2.1.0 manual, registered to the
//! field frame by the two outposts (±3.092, ±3.867 m) and the two bases
//! (±11.68 m) to about 2 cm, and checked against the CAD field markings. One
//! figure pixel is about 3.3 cm, so edges are good to roughly ±5 cm. Floor
//! heights were probed from the default package's collision mesh.
use crate::Team;
use rm_simulator_gameplay::{self as gp, ZoneKind};
use serde::{Deserialize, Serialize};

/// Height of the body centre above a point's floor, in metres, within which
/// the chassis counts as standing on it. Wide enough for the chassis' ride
/// height and suspension travel, and narrow enough that a deck above a tunnel
/// pad or a floor below a highland point does not count. An application
/// setting, not a rule constant.
pub const ZONE_BODY_BAND_M: f64 = 0.45;

/// One buff point card area on the field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneArea {
    /// Buff point kind.
    pub kind: ZoneKind,
    /// Team the point belongs to, or the side it lies on for points any team
    /// may use.
    pub owner: Team,
    /// Pad number within a terrain crossing course; 0 otherwise
    /// ([`gp::Zone::pad`]).
    pub pad: u8,
    /// Floor height under the point, in metres.
    pub floor_m: f64,
    /// Footprint corners in field x and y, in metres, in either winding.
    pub polygon_m: Vec<[f64; 2]>,
}

impl ZoneArea {
    /// The gameplay zone this area reports.
    pub fn zone(&self) -> gp::Zone {
        gp::Zone {
            kind: self.kind,
            owner: crate::referee::game_team(self.owner),
            pad: self.pad,
        }
    }
    /// Whether a chassis body centre at `position_m` stands on this area.
    ///
    /// ```
    /// use rm_simulator_world::{Team, zones::ZoneArea, gameplay::ZoneKind};
    ///
    /// let area = ZoneArea {
    ///     kind: ZoneKind::Base,
    ///     owner: Team::Red,
    ///     pad: 0,
    ///     floor_m: 0.2,
    ///     polygon_m: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    /// };
    /// assert!(area.contains([0.5, 0.5, 0.4]));
    /// assert!(!area.contains([1.5, 0.5, 0.4]));
    /// // Below the floor or high above it does not count.
    /// assert!(!area.contains([0.5, 0.5, 0.1]));
    /// assert!(!area.contains([0.5, 0.5, 0.9]));
    /// ```
    pub fn contains(&self, position_m: [f64; 3]) -> bool {
        let [x, y, z] = position_m;
        if !(self.floor_m..=self.floor_m + ZONE_BODY_BAND_M).contains(&z) {
            return false;
        }
        let mut inside = false;
        let n = self.polygon_m.len();
        for i in 0..n {
            let [xi, yi] = self.polygon_m[i];
            let [xj, yj] = self.polygon_m[(i + n - 1) % n];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
        }
        inside
    }
}

struct Area {
    kind: ZoneKind,
    pad: u8,
    floor_m: f64,
    polygon_m: &'static [[f64; 2]],
}

/// Red's buff points, with red on the +x half. Terrain crossing and central
/// highland points are the ones on red's side of the field.
const RED_AREAS: &[Area] = &[
    Area {
        kind: ZoneKind::Base,
        pad: 0,
        floor_m: 0.0,
        polygon_m: &[
            [12.01, -1.93],
            [9.96, -0.77],
            [9.94, 0.84],
            [11.91, 1.96],
            [13.36, 1.17],
            [13.38, -1.14],
        ],
    },
    Area {
        kind: ZoneKind::Resupply,
        pad: 0,
        floor_m: 0.0,
        polygon_m: &[
            [10.64, 4.96],
            [10.64, 5.62],
            [13.22, 5.62],
            [13.23, 4.60],
            [11.02, 4.60],
        ],
    },
    Area {
        kind: ZoneKind::Resupply,
        pad: 0,
        floor_m: 0.0,
        polygon_m: &[[10.60, 5.72], [10.59, 7.41], [12.70, 7.37], [12.72, 5.76]],
    },
    Area {
        kind: ZoneKind::TrapezoidHighland,
        pad: 0,
        floor_m: 0.4,
        polygon_m: &[[8.88, -4.30], [9.31, -3.31], [11.02, -3.31], [11.03, -4.30]],
    },
    Area {
        kind: ZoneKind::Fortress,
        pad: 0,
        floor_m: 0.0,
        polygon_m: &[
            [6.94, -0.94],
            [6.36, 0.05],
            [6.92, 1.01],
            [8.06, 1.01],
            [8.64, 0.05],
            [8.08, -0.94],
        ],
    },
    Area {
        kind: ZoneKind::ElevatedCrossing,
        pad: 0,
        floor_m: 0.0,
        polygon_m: &[
            [4.87, -1.37],
            [4.84, 1.44],
            [5.51, 1.44],
            [5.71, 1.24],
            [5.74, -1.17],
            [5.54, -1.37],
        ],
    },
    Area {
        kind: ZoneKind::ElevatedCrossing,
        pad: 1,
        floor_m: 0.3,
        polygon_m: &[
            [2.89, -1.37],
            [2.86, 1.20],
            [3.06, 1.44],
            [3.67, 1.44],
            [3.69, -1.37],
        ],
    },
    Area {
        kind: ZoneKind::CentralHighland,
        pad: 0,
        floor_m: 0.3,
        polygon_m: &[
            [2.89, -1.37],
            [2.86, 1.20],
            [3.06, 1.44],
            [3.67, 1.44],
            [3.69, -1.37],
        ],
    },
    Area {
        kind: ZoneKind::CentralHighland,
        pad: 0,
        floor_m: 0.3,
        polygon_m: &[
            [1.48, -4.80],
            [1.46, -3.64],
            [2.89, -1.43],
            [3.69, -1.43],
            [3.66, -1.96],
            [1.68, -4.80],
        ],
    },
    Area {
        kind: ZoneKind::Outpost,
        pad: 0,
        floor_m: 0.3,
        polygon_m: &[
            [3.62, 2.95],
            [3.41, 3.48],
            [2.98, 3.48],
            [2.74, 3.71],
            [2.74, 4.04],
            [2.97, 4.27],
            [3.41, 4.27],
            [3.74, 4.80],
            [2.73, 4.80],
            [2.20, 4.27],
            [2.20, 3.51],
            [2.75, 2.95],
        ],
    },
    Area {
        kind: ZoneKind::Road,
        pad: 0,
        floor_m: 0.0,
        polygon_m: &[[6.34, 4.75], [5.22, 4.75], [5.22, 5.41], [6.34, 5.41]],
    },
    Area {
        kind: ZoneKind::Road,
        pad: 1,
        floor_m: 0.2,
        polygon_m: &[[6.33, 5.57], [5.22, 5.57], [5.22, 6.20], [6.33, 6.20]],
    },
    Area {
        kind: ZoneKind::Tunnel,
        pad: 0,
        floor_m: 0.0,
        polygon_m: &[[5.10, 4.29], [4.32, 4.29], [4.32, 5.01], [5.10, 5.01]],
    },
    Area {
        kind: ZoneKind::Tunnel,
        pad: 1,
        floor_m: 0.0,
        polygon_m: &[[5.09, 5.24], [4.32, 5.24], [4.32, 5.41], [5.09, 5.41]],
    },
    Area {
        kind: ZoneKind::Tunnel,
        pad: 2,
        floor_m: 0.2,
        polygon_m: &[[5.09, 5.61], [4.31, 5.61], [4.31, 6.33], [5.09, 6.33]],
    },
    Area {
        kind: ZoneKind::Tunnel,
        pad: 3,
        floor_m: 0.0,
        polygon_m: &[[0.09, 5.57], [-0.69, 5.57], [-0.69, 6.30], [0.09, 6.30]],
    },
    Area {
        kind: ZoneKind::Tunnel,
        pad: 4,
        floor_m: 0.0,
        polygon_m: &[[-0.88, 5.57], [-1.06, 5.57], [-1.06, 6.30], [-0.88, 6.30]],
    },
    Area {
        kind: ZoneKind::Tunnel,
        pad: 5,
        floor_m: 0.0,
        polygon_m: &[[-1.25, 5.57], [-2.03, 5.57], [-2.03, 6.30], [-1.25, 6.30]],
    },
    Area {
        kind: ZoneKind::LaunchRamp,
        pad: 0,
        floor_m: 0.2,
        polygon_m: &[[4.04, 6.53], [2.79, 6.53], [2.79, 7.42], [4.04, 7.42]],
    },
    Area {
        kind: ZoneKind::LaunchRamp,
        pad: 1,
        floor_m: 0.2,
        polygon_m: &[[-1.90, 6.50], [-3.01, 6.50], [-3.01, 7.39], [-1.90, 7.39]],
    },
];

/// Every RMUC 2026 buff point except the engineer-only Assembly Zone, with red
/// on the +x half as the default CAD layout places it. Blue's points are red's
/// mirrored through the field centre (section 4.1: the field is centrally
/// symmetric).
///
/// ```
/// use rm_simulator_world::{Team, zones, gameplay::ZoneKind};
///
/// let areas = zones::rmuc_2026();
/// let base = |team| {
///     areas
///         .iter()
///         .find(|a| a.kind == ZoneKind::Base && a.owner == team)
///         .unwrap()
/// };
/// assert!(base(Team::Red).contains([11.7, 0.8, 0.2]));
/// assert!(base(Team::Blue).contains([-11.7, -0.8, 0.2]));
/// assert_eq!(areas.iter().filter(|a| a.kind == ZoneKind::Tunnel).count(), 12);
/// ```
pub fn rmuc_2026() -> Vec<ZoneArea> {
    Team::BOTH
        .into_iter()
        .flat_map(|team| {
            let sign = if team == Team::Red { 1.0 } else { -1.0 };
            RED_AREAS.iter().map(move |area| ZoneArea {
                kind: area.kind,
                owner: team,
                pad: area.pad,
                floor_m: area.floor_m,
                polygon_m: area
                    .polygon_m
                    .iter()
                    .map(|[x, y]| [sign * x, sign * y])
                    .collect(),
            })
        })
        .collect()
}
