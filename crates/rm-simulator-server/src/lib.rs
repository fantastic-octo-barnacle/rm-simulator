// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! The authoritative simulation and how to reach it: the CAD package loaded
//! into a `Field`, a `Simulation` that paces it on a host clock, a framed UDP
//! protocol for players (with a JSON-lines TCP transport for protocol tools),
//! and a small HTTP panel for the referee.
//! Nothing here draws; the `rm-simulator` app is one client of this crate,
//! whether it hosts the simulation in-process or connects to
//! `rm-simulator-server`.
#![deny(missing_docs)]

pub mod base_layout;
pub mod cad_assets;
pub mod clock;
pub mod collision_mesh;
pub mod host;
pub mod http;
pub mod layout;
mod lifecycle;
pub mod math;
pub mod net;
pub mod protocol;
pub mod simulation;
pub mod snapshot_codec;

pub mod semantics;

pub mod prediction;

pub mod view;

pub mod input_stream;

pub mod network_stats;

pub mod network_trace;

pub mod owner_stream;

pub mod pacing;

pub mod scripted_link;

pub mod udp_codec;

pub mod udp_snapshot;

pub mod lobby;
