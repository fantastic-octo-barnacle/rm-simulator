// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! CAD-free moving armor and projectiles. No referee, server, renderer or host clock.
use rm_simulator_physics::{
    ArmorTarget, Caliber, Pose, Shot, TICK_NS, TargetFace, TargetFrames, WorldPhysics,
};

fn face(time_ns: u64) -> TargetFace {
    let y_m = 0.04 * (time_ns as f64 * 1e-9 * 2.0).sin();
    TargetFace {
        target: ArmorTarget::Outpost {
            outpost: 0,
            face: 0,
        },
        pose: Pose::yawed([2.0, y_m, 1.0], std::f64::consts::PI),
    }
}
fn main() -> Result<(), &'static str> {
    let mut physics = WorldPhysics::new(&[face(0)], 0.0);
    let mut frames = TargetFrames::new(vec![face(0)]);
    let mut contacts = 0;
    for tick in 0..1_000 {
        let time_ns = tick * TICK_NS;
        if tick % 100 == 0 {
            physics.fire(
                time_ns,
                Pose::at([0.0, 0.0, 1.025]),
                Shot::at_limit(Caliber::Mm17),
                None,
            )?;
        }
        frames.begin(|faces| faces[0] = face(time_ns))?;
        frames.update(|faces| faces[0] = face(time_ns + TICK_NS))?;
        // The caller may detect and score these contacts; this example only observes them.
        contacts += physics.step(time_ns, &frames)?.len();
    }
    assert!(contacts > 0, "the moving armor should intercept the shots");
    println!(
        "1,000 explicit ticks; {} launches; {contacts} raw armor contacts",
        physics.launched()
    );
    Ok(())
}
