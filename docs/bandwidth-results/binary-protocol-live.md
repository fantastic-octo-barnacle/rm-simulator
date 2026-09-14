<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Binary checkpoint live driving test

Protocol 32 makes the measured fine fixed-point candidate the default for
periodic UDP world checkpoints. Both peers must run the updated build. This
integration is separate from the initial experiment. The interactive loopback
driving test has been completed and the user reported that it looks good.

## Drive through the network path

Build both binaries, then use two terminals from the repository root:

```sh
cargo build --locked -p rm-simulator-server -p rm-simulator-app --bins
```

First terminal:

```sh
RM_NET_TRACE_DIR=/tmp/rm-binary-host just server --listen 127.0.0.1:7700
```

Second terminal:

```sh
RM_NET_TRACE_DIR=/tmp/rm-binary-client just run --connect 127.0.0.1:7700 \
  --name binary-test --team red --network-stats detailed
```

Use a remote connection even on the same machine: a local in-process owner
does not exercise the UDP world-checkpoint encoding. Drive, turn, aim and fire;
try ramps and contacts. The P/F3 network panel exposes traffic and prediction
diagnostics. The headless host's referee panel is at `http://127.0.0.1:7780`
unless a different `--http` address was supplied. Stop the app and server before
changing comparison settings.

For the previous JSON + trained-dictionary baseline, restart the host with:

```sh
RM_NET_SNAPSHOT=json RM_NET_CODEC=zstd-dict \
  RM_NET_TRACE_DIR=/tmp/rm-json-host just server --listen 127.0.0.1:7700
```

The client auto-detects the received format. For the old DEFLATE checkpoint
baseline, use `RM_NET_SNAPSHOT=json RM_NET_CODEC=deflate`. These settings are
frozen on first use, so changing them requires restarting the host.
`RM_NET_CODEC` still governs other compressed message kinds; binary checkpoints
always use their own dictionary at ZSTD level 3. Testing with
`RM_NET_FULL_CHECKPOINTS=1` bypasses compact periodic checkpoints entirely.

## What changes

The live UDP encoder retains its existing full/independent/delta selection,
acknowledgement handling, two-baseline cap, epoch resets, lost-ack retries and
recovery. Quantization happens on a copied checkpoint before the baseline is
stored. The decoder pins the exact wire-grid values and normalizes quaternion
components only in the delivered state, so future deltas reference identical
values on both peers. Invalid rotations are rejected before a baseline is pinned.

Position resolution is 1 mm for chassis state and 0.01 mm for projectiles;
velocity resolution is 0.01 m/s and 0.0001 m/s respectively. The remaining
precision choices and measured replay errors are documented in the
[experiment](binary-protocol.md#precision-assumptions). Out-of-range values
retain their existing precision. Extreme projectiles that require the legacy
full checkpoint fallback retain that full state.

The host physics, rule state, commands, timestamps, owner anchors, reliable
confirmations and TCP snapshots keep their previous precision. Binary frames
use RMBZ with the new dictionary, not the JSON dictionary's RMZ2 marker.
Dictionary and encoding changes require a further protocol version bump and
fresh training/evaluation; the protocol 32 asset is checked against its recorded
hash in tests.

The existing end-to-end app harness passes with binary checkpoints under clean
delivery, loss, reordering, duplication and blackout recovery. Interactive CAD
terrain/contact behavior remains the purpose of this test; the earlier
synthetic bandwidth and replay measurements do not replace it.

The complete `just verify` suite passed with binary checkpoints as the default,
including workspace tests/doctests, clippy, Python harness checks, dependency
boundaries and license checks. Both app and server binaries were built for the
interactive test. The user completed a loopback GNS driving check with the CAD
field and reported that it looks good; this does not replace the targeted
precision/contact limitations documented in the experiment.
