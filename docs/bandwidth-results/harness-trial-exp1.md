<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Real end-to-end harness trial: experiment 1 candidate vs baseline

One matched pair of `rm-simulator-server` + `rm-simulator` release binaries run
through `scripts/network-harness.py` over real UDP with the real CAD package, to
test the in-process attribution against a live trial.

## Builds

| Side | Revision | Branch | Notes |
|---|---|---|---|
| Candidate | `8eb429d` | `perf/bw-exp1` | owner configuration referenced by identity, `PROTOCOL_VERSION = 28` |
| Baseline | `3a41fcc` | `perf/bw-base` | probe-only commit; production wire unchanged, `PROTOCOL_VERSION = 27` |

Candidate release binaries were the ones built from `worktrees/exp1`
(`target/release`, shared target directory). The matched baseline pair is built
from `worktrees/base` into a separate `target-baseline` directory so the two
binaries cannot be confused. Both are built with the lockfile, same profile.

## Scenario

`scripts/network-scenarios/direct-single.json`: one remote pilot, 12 s active,
1 s warmup, 2 s recovery, movement plus firing, no configured impairment. The
harness proxy forwards real UDP payloads; rates below are forwarded proxy
payload bytes over the scenario's active plus recovery window, so they are
comparable to the documented NET-001 proxy-payload figures but not identical in
duration and workload.

## Candidate result (passed)

Command:

```sh
python3 scripts/network-harness.py scripts/network-scenarios/direct-single.json \
  --output /tmp/rm-bw-exp1-trial \
  --server target/release/rm-simulator-server --app target/release/rm-simulator \
  --cad-assets local-assets/field
```

Status `passed`, all seven checks green: 48 active samples with no gaps, app RTT
p50 4.62 ms, maximum checkpoint gap 20.7 ms, remote underruns 0, unresolved shot
outcomes 0, robot displacement 6.58 m, 89 confirmed launches.

Proxy payload totals over 14.17 s of the run:

| Direction | Packets | Payload bytes | Payload kbps |
|---|---:|---:|---:|
| downstream | 1199 | 1,084,912 | 612.6 |
| upstream | 776 | 155,760 | 87.9 |

The proxy's accounted (header-inclusive) downstream figure is 1,118,484 bytes,
631.4 kbps.

## Matched baseline result

The same scenario, same harness and same CAD package against the production
wire (`perf/bw-base`, `3a41fcc`, binaries built into `target-baseline`):

| Direction | Payload bytes | Payload kbps |
|---|---:|---:|
| downstream | 1,253,784 | 707.3 |
| upstream | 155,600 | 87.8 |

Both runs passed every check with 89 confirmed launches and 6.58 m of robot
displacement, so the workloads match; the binary hashes in each run's
`manifest.json` differ, as they must.

## Matched-pair result

| Direction | Baseline | Candidate | Change |
|---|---:|---:|---:|
| downstream | 707.3 kbps | 612.6 kbps | **−94.7 kbps, −13.4%** |
| upstream | 87.8 kbps | 87.9 kbps | unchanged |

This is the acceptance evidence for experiment 1. It confirms in a live trial
what the in-process probe predicted: the repeated owner configuration cost about
100 kbps per remote pilot. Upstream is unchanged, as expected for a downstream
owner-stream change.

## Reading

- The trial proves the whole path works with the experiment 1 wire change on
  real UDP and the real field: the pilot moves, launches 89 rounds, resolves
  every shot outcome, and holds a 20.7 ms worst checkpoint gap.
- The measured 612.6 kbps downstream and the baseline 707.3 kbps both sit in
  the range the in-process probe predicted for the remote path (exp 1 candidate
  `fire` 640.4 kbps, baseline `fire` 741.0 kbps), which is the cross-check the
  in-process instrument needed.
- The measured saving (−94.7 kbps) matches the in-process prediction
  (−100.6 kbps) within the difference between the scenario workloads.
- Downstream still needs a 3x reduction to reach 200 kbps, and upstream is
  already near the earlier 100 kbps figure, consistent with the in-process
  conclusion that cadence, world-stream size and the remaining anchor cost are
  the open levers.
- The harness reads `protocol_source` from the coordinating checkout, so its
  manifest reports 27 for both runs even though the candidate is
  `PROTOCOL_VERSION = 28`. The binary hashes are the reliable identity here.
