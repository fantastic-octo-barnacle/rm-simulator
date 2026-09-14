<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Bandwidth experiment plan and log

Working record for executing every experiment in
[bandwidth-experiments.md](../bandwidth-experiments.md). One isolated Git
worktree and branch per experiment, measurement first, and only the
experiments that show a win get a full scripted-link/impairment run.

## Integration follow-up (protocol 29)

Experiments 0, 1 and 7 are integrated on the coordinating branch as separate
commits: `6d947f0` (attribution), `d9dc32b` (owner configuration references), and
`7df316f` (input compaction and combined protocol version). The canonical
probe uses RMI3 accounting; the combined wire version is 29 because experiments
1 and 7 independently used version 28 for incompatible contracts. The isolated
results below remain historical evidence, not a combined on-wire measurement.
The integrated workspace passed `just verify` (formatting, all-feature check,
clippy, workspace tests/doctests, Python harness tests, crate boundaries, MPL
compliance and dependency policy). Cadence and pacing defaults are unchanged.
The full impaired-link acceptance
matrix and populated outcome workloads remain outstanding; NET-001 stays open.

## Ground rules used for every experiment

- Base revision `7d07f1c` (`perf/network-bandwidth`). Branch `perf/bw-expN` on
  worktree `worktrees/expN`.
- One shared `CARGO_TARGET_DIR` so external dependency artifacts are compiled
  once; workspace crates recompile per worktree. Never `cargo clean`.
- Every experiment measures the **same** probe workloads before and after, and
  reports offered and delivered bytes separately. Production, pacing and
  delivery counters are never summed.
- The fast loop is the in-process probe (`cargo test -p rm-simulator-server
  bandwidth -- --nocapture`). Real server/app binaries over UDP are only built
  for a candidate that already shows a win in-process.
- A results file per experiment: `docs/bandwidth-results/exp-N-*.md`.
- Experiment 8 keeps the same measurement discipline but runs on
  `feat/zstd-compression` in `.worktrees/zstd-compression` rather than a
  `perf/bw-expN` worktree, because it also carries the selectable codec the
  branch exists for.

## Measurement instrument

`crates/rm-simulator-server/src/bandwidth_probe.rs` (test-only) drives the
production `PeerCodec` and `ClientCodec` over no socket on a `ManualTime`
clock at 2 ms steps and the real 32 ms publication period. Workloads: `idle`,
`drive`, `fire`, `twelve`. It reports per direction and class:

- downstream application bytes/s and kbps, datagrams, world fragments, complete
  checkpoints, incomplete frames, stale checkpoints, anchors;
- owner / world / control byte split, owner and world update counts;
- upstream bytes/s, batches, carried input frames, control datagrams;
- raw checkpoint JSON, owner anchor size, section sizes, projectile/chassis/hit
  and shot-result counts, encoder raw/framed/independent/selected bytes;
- loop, host-encode and client-decode CPU.

It asserts its own determinism, accounting consistency and workload
sensitivity. Canonical revision: `48f20e6` on `perf/bw-exp0`.

## Harness hazards found while running the experiments

- **Shared target directory.** Every worktree uses
  `CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target`, and the server
  crate's lib-test artifact name does not include the workspace path, so a
  concurrent `cargo test` can silently run another worktree's build. The
  reliable workaround is a private per-package profile hash
  (`--config 'profile.dev.package.rm-simulator-server.codegen-units=16'`, and a
  `debug` override), plus confirming the compile line names the worktree and the
  output carries a marker unique to that experiment.
- **`git stash` is repository-global across worktrees.** Two experiments
  corrupted each other's working trees through `stash push`/`pop`. Use file
  backups plus `git checkout HEAD -- <paths>`, or commit immediately.
- **Probe workload gap.** The canonical `fire`/`twelve` workloads launch shots
  but never record a `ShotResult` or land a hit, so `shot_results` and
  `state.hits` are empty in every frame. Any experiment about outcome history
  must add a populated scenario; experiment 6 did, and measured 20% of the
  independent frame in that scenario against zero in the canonical one.

## Experiment log

| # | Experiment | Branch | Status | Result |
|---|---|---|---|---|
| 0 | Attribute the bytes | `perf/bw-exp0` | **done** | remote pilot 368/454/741/955 kbps down (idle/drive/fire/twelve); owner anchor is a fixed 211.8 kbps (846 B at 31.25 Hz, 408 B of it repeated config) |
| 1 | Stop repeating owner configuration | `perf/bw-exp1` | **done** | anchor 846 → 444 B; owner 211.8 → 110.8 kbps; every remote workload −100.6 kbps; residual 111 kbps |
| 2 | Identity-based world deltas | `perf/bw-exp2` | **done** | correct and lossless but selected 0 times in 44,597 array decisions; 0 B / 0 kbps on every workload. Moving values make every projectile an update, and the small chassis array already wins on index patches |
| 3 | Lossless binary checkpoint | `perf/bw-exp3` | **done** | binary `state.chassis`: independent frame −12…21%, selected stream only −1.2…3.8%; deflate level 1→4 cuts the independent frame 12–14% for negligible CPU and should be measured on the selected stream |
| 4 | Baseline rotation sweep | `perf/bw-exp4` | **done** | keep 32; 8/16 lose everywhere, 64 wins only idle/drive while losing 22.6 kbps on `twelve`; size-triggered policy rejected (inert 70% of the time, thrashes `fire` +38…50%) |
| 5 | Cadence experiments | `perf/bw-exp5` | **done** | 31.25/15.625 Hz gives −20.5…−36.8%; 15.625/15.625 gives ≈−49%; 62.5/15.625 is a loss until the anchor shrinks; 3 lost checkpoints still inside the 300 ms context gate |
| 6 | Outcome recovery repetition | `perf/bw-exp6` | **done** | zero share in the canonical workloads (sections empty); populated scenario 1015 B/frame ≈ 20%, identity splice −6.6% there; recommendation: leave it in place |
| 7 | Upstream batch compaction | `perf/bw-exp7` | **done** | RMI3 batch: upstream 36.5→27.0 idle, 73.4→56.6 drive, 76.4→59.6 fire kbps (−22…−26%); no pacing change justified |
| 8 | ZSTD with a trained dictionary | `feat/zstd-compression` | **done** | selectable codec; DEFLATE unchanged and still the default. Plain ZSTD saves up to 38% on the selected stream; `zstd-dict-3` cuts the selected stream 30–58% and the independent envelopes 58–82% against `deflate-1`, halves fragments on `drive` and cuts them 26% on `fire`, and lowers CPU in both directions. Full production probe, remote cadence: downstream 268.1→196.6 idle, 671.7→524.3 fire, 877.9→695.8 twelve kbps (−21…−27%), no checkpoint lost; upstream +1.5…3.3 kbps. Out of sample: a leave-one-out dictionary is only 8–22% better. See [exp-8](exp-8-zstd-dictionary.md) |

## Headline consequences so far

- The remote downstream is dominated by the world checkpoint once the field
  moves (`fire` 528.6 of 741.0 kbps; `twelve` 742.8 of 955.1).
- The owner anchor is a fixed 846 byte datagram in every remote world frame:
  211.8 kbps even when idle. Removing the repeated configuration (exp 1) halves
  it to 111 kbps. At a 200 kbps budget and 31.25 Hz publications the whole
  allowance is 800 bytes per publication including overhead, so the owner stream
  and the world stream together cannot fit without changing cadence or size.
- Cadence (exp 5) is the largest single lever measured so far: halving the world
  publication rate removes 20-37% of downstream while keeping three lost
  checkpoints inside the 300 ms context gate.
- Upstream (exp 7) is already within the 10 KiB/s limited budget after
  compaction; no pacing change is justified.
- Experiment 1 (anchor) + experiment 5 (cadence) compose: at 15.625/15.625 with
  the referenced configuration the owner stream would be about 55 kbps instead
  of 211.8, on top of the world-stream halving. That composition has not been
  measured and is the first thing a follow-up should test.
- The negative results narrow the remaining work usefully. Experiment 2 shows
  the world delta does not need restructuring: with lossless values, a keyed
  patch is never smaller than the index patch because moving balls are updates
  in both. Experiment 3 shows re-encoding the *selected* chassis bytes is worth
  little because acknowledged deltas already skip the unchanged parts. What is
  left in the world stream is the moving projectile position and velocity
  (520/530 B per independent frame), which is a precision or binary question,
  and was explicitly deferred by the handoff until the lossless options were
  exhausted.
- Experiment 6's zero share and experiment 2's zero selection both come from
  the same gap: the canonical probe's `fire`/`twelve` workloads launch shots but
  record no `ShotResult` and land no hit. Any future experiment that touches
  outcome history or projectile churn must add a populated scenario first
  (experiment 6's `bandwidth_attribution_populated_outcomes` is a starting
  point).

- Experiment 8 is the first codec change that beats DEFLATE by more than a few
  percent on the *selected* stream: a dictionary-trained ZSTD cuts it 30–58%.
  It is also the first result that lowers CPU in both directions, and it
  composes with the acknowledged-baseline scheme (242 of 250 frames stayed
  deltas). It still does not reach 200 kbps on the firing workloads, so it
  stacks with cadence rather than replacing it.

## Real end-to-end acceptance run

Experiment 1 was taken through the real harness as a matched pair of release
binaries over real UDP and the real field
([harness-trial-exp1.md](harness-trial-exp1.md)): baseline 707.3 kbps
downstream against the candidate's 612.6 kbps, −13.4%, with both runs passing
every scenario check (89 confirmed launches, 6.58 m displacement, 20.7 ms worst
checkpoint gap). The measured saving matches the in-process prediction
(−100.6 kbps) and cross-checks the instrument. The other experiments were not
trialled because they show no standalone win (2, 3, 6) or are tuning that the
in-process sweep already settles (4). The next candidate for a harness pair is
experiment 8's dictionary-trained ZSTD using the recommended `zstd-dict-3`
configuration; its fragment reduction is already measured, and only the
loss-recovery benefit of fewer fragments can be validated under loss. Cadence
(5) remains a later candidate.
