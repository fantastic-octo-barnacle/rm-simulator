<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Experiment 4: tune acknowledged baseline rotation

Sweep of the acknowledged-baseline rotation lifetime for the UDP delta lane,
step 4 of [bandwidth-experiments.md](../bandwidth-experiments.md), against the
attribution baseline recorded in `perf/bw-exp0`'s
`docs/bandwidth-results/exp-0-attribution.md` (commit `48f20e6`).

**Recommendation: keep the shipped 32 encoded frames.** 64 is very slightly
better on the light workloads and clearly worse on `fire`/`twelve`; 8 and 16 are
worse everywhere; and a size-triggered rotation policy is either inert or
strictly harmful. No setting gained from loss.

## Revision and commands

- **Base revision:** `7d07f1c` on `perf/bw-exp4` (the branch did not yet carry
  the probe; see "Instrument" below).
- **Measured tree:** the commit this file and the parameterised encoder are in.
  Get it with `git log --oneline -1 -- docs/bandwidth-results/exp-4-rotation.md`.
- **`git status --short` at measurement time:** only the files of this commit
  (`bandwidth_probe.rs`, `udp_snapshot.rs`, `udp_codec.rs`, `network_stats.rs`,
  `lib.rs`, this file) plus the untracked `KNOWN_ISSUES.md` and
  `docs/bandwidth-experiments.md` that every bandwidth worktree carries.

### Instrument

The branch carried no probe. `crates/rm-simulator-server/src/bandwidth_probe.rs`
was taken verbatim from `perf/bw-exp0` commit `48f20e6` and registered in
`lib.rs` with `#[cfg(test)] mod bandwidth_probe;`; `udp_codec.rs` needed back
only exp0's `#[cfg(test)] encoding_stats` accessor. `udp_snapshot.rs` is
byte-identical to exp0's on this branch, so the probe compiles unchanged. The
probe was then extended, not rewritten:

- `Encoder` gained [`RotationPolicy`], a `with_policy` builder, a `policy`
  accessor, and counters for `Full` proposals, `Retire` messages, resends and
  delta bytes. The retired `count >= 32` literal became
  `policy.lifetime_frames`, whose default is still 32.
- The probe gained `RotationConfig`, `run_rotated`, `rotation_report` and a
  per-frame delta/independent histogram; and `lossy`/`loss_report`, which drive
  the same host/client pair across `scripted_link::Link`.
- `EncodingStats` gained `delta_bytes`, `baseline_full_frames`,
  `baseline_retires` and `baseline_retire_resends`, and
  `PeerCodec::encoding_stats` fills them. The delivery report is additive, so
  the wire schema is unchanged.

### Build provenance (why this mattered)

Everything shares `CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/target`,
and the lib-test artifact is `rm_simulator_server-<metadata-hash>` for every
worktree. A concurrent sibling build **did** overwrite this worktree's artifact
between build and run: the first capture ran a foreign binary with three probe
tests instead of nine. A `debug=1` config override did not fix it. Nothing below
comes from that capture.

The captures reported here use a worktree-local target directory for this
worktree's own crate:

```sh
CARGO_TARGET_DIR=/Users/hxyulin/dev/RM/rm-simulator/worktrees/exp4/target-exp4 \
  cargo test -p rm-simulator-server --locked --no-run --message-format=json
cp target-exp4/debug/deps/rm_simulator_server-<hash> target-probe/probe-lib-tests
strings target-probe/probe-lib-tests | grep -c rotation_sweep_all_workloads   # 1
target-probe/probe-lib-tests bandwidth_ --nocapture --test-threads=1
```

`target-exp4/` is a harness workaround for the shared-target hazard, not a code
change; it is removed after the experiment. The compile line names
`rm-simulator-server v0.1.0 (/Users/hxyulin/dev/RM/rm-simulator/worktrees/exp4/crates/rm-simulator-server)`,
the capture contains `probe cadence=` and this experiment's `rotation=` values,
and the rows differ between settings (asserted by `rotation_sweep_all_workloads`,
which fails if any two fixed lifetimes report identical bytes). The verified
binary is `target-probe/probe-lib-tests`, SHA-256 `75f2cea4a74fb9d2…`, and the
raw captures are `/tmp/bw-exp4-final.txt` (probe), `/tmp/bw-exp4-snapshot-tests.txt`
(encoder invariants) and `/tmp/bw-exp4-loss.txt` (loss). Nine probe tests pass;
the whole server crate is green (178 lib tests + 28 doctests).

## Instrument soundness

Before interpreting the sweep, the attribution was checked against exp0 on the
same workloads. Reproduced exactly, with 32 as the lifetime:

| Workload | Down B/s | Down kbps | exp0 kbps | Selected B | Independent B | Full | Retire | Complete ckpts |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `idle` | 46037.8 | 368.3 | 368.3 | 187948 | 690898 | 10 | 9 | 313 |
| `drive` | 56721.9 | 453.8 | 453.8 | 294789 | 764714 | 10 | 9 | 313 |
| `fire` | 92623.8 | 741.0 | 741.0 | 643140 | 1125899 | 10 | 9 | 313 |
| `twelve` | 119391.4 | 955.1 | 955.1 | 906406 | 1934846 | 10 | 9 | 313 |

`produced_world_updates`, `anchors`, `selected_bytes`, `independent_bytes` and
`world_fragments` are all non-zero, and `idle`/`drive`/`fire`/`twelve` differ.
The four remote figures and all four kbps values match exp0 byte for byte, which
is the strongest available evidence that the capture is this tree's build.

## Sweep: fixed lifetimes 8 / 16 / 32 / 64

Remote pilot (32 ms world publications), unlimited pacer, 10 s per workload,
one `Full` proposal per rotation plus its `Retire` exchange. `B/ckpt` is
delivered world bytes per complete checkpoint; `sel/ind` is the compressed
selected stream against the independent alternative.

| Workload | Lifetime | Down B/s | Down kbps | World B/s | Selected B | Delta B | Independent B | Full | Retire | Resend | Complete | B/ckpt | Frags |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `idle` | 8 | 51217.0 | 409.7 | 24457.3 | 236320 | 147719 | 690898 | 40 | 38 | 0 | 313 | 781.4 | 393 |
| `idle` | 16 | 47675.3 | 381.4 | 21059.6 | 203183 | 158964 | 690898 | 20 | 19 | 0 | 313 | 672.8 | 353 |
| `idle` | **32** | **46037.8** | **368.3** | **19494.1** | **187948** | **165932** | 690898 | **10** | **9** | 0 | 313 | **622.8** | **333** |
| `idle` | 64 | 45810.2 | 366.5 | 19302.0 | 186237 | 175316 | 690898 | 5 | 4 | 0 | 313 | 616.7 | 323 |
| `drive` | 8 | 61299.0 | 490.4 | 34539.3 | 337140 | 239096 | 764714 | 40 | 38 | 0 | 313 | 1103.5 | 393 |
| `drive` | 16 | 58401.5 | 467.2 | 31785.8 | 310445 | 261604 | 764714 | 20 | 19 | 0 | 313 | 1015.5 | 353 |
| `drive` | **32** | **56721.9** | **453.8** | **30178.2** | **294789** | **270583** | 764714 | **10** | **9** | 0 | 313 | **964.2** | **333** |
| `drive` | 64 | 56206.7 | 449.7 | 29698.5 | 290202 | 278316 | 764714 | 5 | 4 | 0 | 313 | 948.8 | 323 |
| `fire` | 8 | 97099.9 | 776.8 | 70340.2 | 685132 | 540093 | 1125899 | 40 | 38 | 0 | 313 | 2247.3 | 870 |
| `fire` | 16 | 94290.9 | 754.3 | 67675.2 | 658839 | 586936 | 1125899 | 20 | 19 | 0 | 313 | 2162.1 | 853 |
| `fire` | **32** | **92623.8** | **741.0** | **66080.1** | **643140** | **607755** | 1125899 | **10** | **9** | 0 | 313 | **2111.2** | **841** |
| `fire` | 64 | 92124.6 | 737.0 | 65616.4 | 638566 | 621502 | 1125899 | 5 | 4 | 0 | 313 | 2096.4 | 838 |
| `twelve` | 8 | 128070.4 | 1024.6 | 101310.7 | 989566 | 741063 | 1934846 | 40 | 38 | 0 | 313 | 3236.8 | 1121 |
| `twelve` | 16 | 122305.4 | 978.4 | 95689.7 | 934049 | 811005 | 1934846 | 20 | 19 | 0 | 313 | 3057.2 | 1088 |
| `twelve` | **32** | **119391.4** | **955.1** | **92847.7** | **906406** | **846328** | 1934846 | **10** | **9** | 0 | 313 | **2966.4** | **1051** |
| `twelve` | 64 | 122208.7 | 977.7 | 95700.5 | 933842 | 904707 | 1934846 | 5 | 4 | 0 | 313 | 3057.5 | 1103 |

Aggregate of the four workloads, which is how the per-player budget is spent:

| Lifetime | Total B/s | Total kbps | vs 32 | Total Full | Total Retire | Total resends |
|---|---:|---:|---:|---:|---:|---:|
| 8 | 333687.0 | 2669.5 | +7.1% | 160 | 152 | 0 |
| 16 | 320668.1 | 2565.3 | +2.9% | 80 | 76 | 0 |
| **32** | **311517.2** | **2492.1** | — | **40** | **36** | 0 |
| 64 | 306887.0 | 2455.1 | −1.5% | 20 | 16 | 0 |

What the sweep says:

- **32 was already the right fixed lifetime, and it is the best on the heavy
  workloads.** `twelve` is worst at 64 (+2.4%, +22.6 kbps) and best at 32;
  `fire` is worst at 8 (+5.4%) and best at 32/64. Against 8 and 16, 32 wins
  everywhere: `twelve` −6.8%/−2.4%, `fire` −4.6%/−1.8%, `drive` −7.5%/−2.9%,
  `idle` −10.1%/−3.4%.
- **64 only wins where the world stream is small.** `idle` −0.5%, `drive`
  −0.9%; those are 3.0 and 4.9 kbps of a 368/454 kbps stream, while its losses
  on `fire`/`twelve` are 0.6 and 22.6 kbps. The −1.5% aggregate for 64 is bought
  entirely from the two workloads that do not need help.
- **Shorter lifetimes do not shrink the checkpoint, they multiply the
  anchors.** `bytes/checkpoint` falls as the lifetime shortens (idle 781.4 →
  616.7, twelve 3236.8 → 3057.5) but total bytes rise: every extra world
  publication also carries an owner anchor (`down_bytes` includes them; see
  exp0's 211.8 kbps anchor floor), so 4× the rotations means 4× the prologue
  datagrams. Delta bytes alone would have preferred 8; the checkpoint metric is
  the one that decides.
- Each rotation is one `Full` plus one `Retire`, and 10 s at 32 encodes 313
  frames: exactly `ceil(313 / 32) = 10` proposals, 9 retirements (the last
  proposal has no older baseline to retire). No `Retire` was ever resent, so the
  acknowledgement path never stalled on a lossless link.

## Size-triggered rotation, measured against 32/64

Policy: propose a new baseline as soon as the previous frame's delta reached a
configured percentage of that frame's independent encoding, capped by the
frame-count lifetime. The trigger is evaluated while the old baseline is still
cheap to replace, and only when no proposal or retirement is outstanding.

Per-frame delta shares (delta ÷ that frame's independent bytes) show why the
threshold matters. Buckets are `0–4 / 5–9 / 10–19 / 20–34 / 35–49 / 50–69 /
70–89 / 90+` percent, at lifetime 64:

| Workload | Frames in each share bucket | Largest share seen |
|---|---|---|
| `idle` | 5 / 0 / 0 / 308 / 0 / 0 / 0 / 0 | 20–34% |
| `drive` | 5 / 0 / 0 / 23 / 285 / 0 / 0 / 0 | 35–49% |
| `fire` | 5 / 0 / 0 / 1 / 53 / 254 / 0 / 0 | 50–69% |
| `twelve` | 5 / 0 / 0 / 1 / 216 / 91 / 0 / 0 | 50–69% (88 at 70–89% when re-based) |

Measured policies, all on top of a 64-frame cap:

| Workload | Policy | Down B/s | Selected B | Delta B | Full | Retire | B/ckpt |
|---|---|---:|---:|---:|---:|---:|---:|
| `fire` | 64 | 92124.6 | 638566 | 621502 | 5 | 4 | 2096.4 |
| `fire` | 64 + 70% | 92124.6 | 638566 | 621502 | 5 | 4 | 2096.4 |
| `fire` | 64 + 55% | 127081.1 | 969003 | 153771 | 210 | 208 | 3165.7 |
| `fire` | 64 + 45% | 138532.5 | 1076075 | 40001 | 278 | 276 | 3515.7 |
| `twelve` | 64 | 122208.7 | 933842 | 904707 | 5 | 4 | 3057.5 |
| `twelve` | 64 + 70% | 122208.7 | 933842 | 904707 | 5 | 4 | 3057.5 |
| `twelve` | 64 + 55% | 122433.5 | 935985 | 907044 | 5 | 4 | 3064.7 |
| `twelve` | 64 + 45% | 196104.6 | 1644195 | 185756 | 217 | 215 | 5369.3 |
| `idle` | 64 | 45810.2 | 186237 | 175316 | 5 | 4 | 616.7 |
| `idle` | 64 + 70/55/45% | 45810.2 | 186237 | 175316 | 5 | 4 | 616.7 |
| `drive` | 64 | 56206.7 | 290202 | 278316 | 5 | 4 | 948.8 |
| `drive` | 64 + 70/55/45% | 56206.7 | 290202 | 278316 | 5 | 4 | 948.8 |

The policy either does nothing or thrashes:

- **Inert above the observed share.** At 70% no workload ever triggers, so
  idle/drive/fire/twelve all reproduce plain 64 exactly. The largest share any
  workload reaches before re-basing is 88% (`twelve` at lifetime 70), so a
  threshold near 90% would fire only on the frames where a delta already costs
  what a fresh full frame costs — the break-even point, not a win.
- **Thrashing below it.** At 55% on `fire` the trigger fires 210 times in 313
  frames (+38% bytes/s); at 45% it fires 278 times (+50%) and the delta bytes
  collapse to 40001 B because almost every frame is a full one. A threshold
  inside the natural spread of the share produces a rotation storm, and each
  rotation also costs its `Retire` exchange, so the failure is superlinear in
  proposal count.
- **Nowhere does it beat 32.** Best size-triggered rows are byte-identical to
  64; 64 loses to 32 on `fire`/`twelve`, so the policy has no win to inherit.

There is no hysteresis parameter in this design, and one would not help: a
cooldown only moves a thrashing threshold toward an inert one, and the inert
side cannot beat 32. The size trigger is therefore **not adopted**, and the
`size_trigger_percent` field stays `None` in `RotationPolicy::default`.

## Loss, reordering and blackout

Same host/client pair, every datagram crossing the server's own
`scripted_link::Link` on the manual clock: one seeded SplitMix64 stream per
direction, scripted loss on the unreliable lane, and a blackout window where the
unreliable lane drops and the reliable lane delays past the end. Workload
`fire` (the heaviest world stream), 12 s per row, outage from 3.000 s. `stall`
is the client-visible gap from the last complete checkpoint before the outage to
the first complete one after it; `recov bytes` is the downstream traffic over
that same window.

| Profile | Lifetime | Offered | Delivered | Drop down/up | Down B/s | Down ratio vs lossless | Stall ms | Recov bytes | Complete | Incomplete | Full | Retire | Resend | Selected B | Delta B |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| none | 8 | 1484 | 1484 | 0 / 0 | 98402.2 | 1.000 | 32 | 2797 | 375 | 0 | 47 | 46 | 0 | 837950 | 665172 |
| none | 16 | 1441 | 1441 | 0 / 0 | 95733.9 | 1.000 | 32 | 2804 | 375 | 0 | 24 | 23 | 0 | 808007 | 720260 |
| none | **32** | 1415 | 1415 | 0 / 0 | **94036.0** | 1.000 | 32 | 2807 | 375 | 0 | 12 | 11 | 0 | 788790 | 745485 |
| none | 64 | 1405 | 1405 | 0 / 0 | 93489.5 | 1.000 | 32 | 2807 | 375 | 0 | 6 | 5 | 0 | 782744 | 761722 |
| 1% loss | 8 | 1484 | 1464 | 20 / 4 | 97065.2 | 1.014 | 32 | 2782 | 361 | 14 | 47 | 46 | 0 | 839363 | 666485 |
| 1% loss | 16 | 1440 | 1421 | 19 / 3 | 94986.7 | 1.008 | 32 | 2776 | 358 | 14 | 25 | 21 | 0 | 813528 | 722257 |
| 1% loss | **32** | 1421 | 1402 | 19 / 3 | **93308.3** | 1.008 | 32 | 2795 | 362 | 12 | 14 | 11 | 0 | 792779 | 742938 |
| 1% loss | 64 | 1405 | 1385 | 20 / 4 | 92242.8 | 1.013 | 32 | 2796 | 360 | 15 | 6 | 5 | 0 | 784038 | 762985 |
| 5% loss | 8 | 1473 | 1405 | 68 / 38 | 94413.6 | 1.041 | 64 | 2813 | 318 | 46 | 47 | 34 | 0 | 846203 | 673463 |
| 5% loss | 16 | 1446 | 1382 | 64 / 33 | 92232.7 | 1.037 | 32 | 2792 | 326 | 40 | 28 | 19 | 0 | 818846 | 716513 |
| 5% loss | **32** | 1416 | 1351 | 65 / 41 | **90051.8** | 1.042 | 32 | 2833 | 331 | 43 | 12 | 11 | 0 | 792782 | 749441 |
| 5% loss | 64 | 1407 | 1343 | 64 / 36 | 89504.1 | 1.043 | 32 | 2831 | 328 | 46 | 7 | 5 | 0 | 787685 | 762622 |
| 0.5 s blackout | 8 | 1475 | 1423 | 52 / 31 | 93123.1 | 1.057 | **544** | 2811 | 358 | 0 | 47 | 44 | 0 | 823234 | 652754 |
| 0.5 s blackout | 16 | 1433 | 1381 | 52 / 31 | 90584.4 | 1.057 | **544** | 2912 | 358 | 0 | 25 | 22 | 0 | 794935 | 704861 |
| 0.5 s blackout | **32** | 1407 | 1355 | 52 / 31 | **88996.5** | 1.057 | **544** | 2825 | 358 | 0 | 14 | 11 | 0 | 777044 | 727121 |
| 0.5 s blackout | 64 | 1392 | 1344 | 48 / 31 | 88417.0 | 1.057 | **544** | 2824 | 359 | 0 | 6 | 5 | 0 | 767467 | 746698 |

("Down ratio" is this row's delivered bytes divided by the same lifetime's
lossless row. Values are 1.0–1.06, which is the burst the handoff asks this
table to expose.)

What the loss table says:

- **No setting turns loss into excess full-frame traffic.** Across all twelve
  impaired rows the delivered-byte total is within 1.4–5.7% of the same
  lifetime's lossless total (1% loss 1.008–1.014×, 5% loss 1.037–1.043×, the
  blackout 1.057× on every lifetime). Nothing loses 40% of its traffic and
  nothing doubles: the loss table shows no full-frame storm, and the rank order
  of the settings is essentially unchanged by loss (32 stays best on `fire`
  under every profile; only the last decimal of the 32-vs-64 gap moves).
- **The stall is a property of the outage, not of the rotation policy.** 1% loss
  recovers within one publication period (32 ms) at every lifetime; 5% costs at
  most one extra publication (64 ms at lifetime 8, 32 ms otherwise); the
  blackout produces a 544 ms stall at all four lifetimes — the 500 ms outage
  plus the next publication. No lifetime recovered faster or slower.
- **Recovery cost is the same for every lifetime** (2.78–2.91 kB, one
  independent frame plus its fragments). Under loss the recovery path is an
  independent frame or a `Full` re-baseline, so it does not depend on the
  lifetime that was in force.
- **The independent fallback and the acknowledgement rule held throughout.**
  `incomplete_frames > 0` only for the loss profiles (250 ms fragment lifetime)
  and always 0 for the blackout; `missing` is 0 in all 16 rows because the
  encoder's `recover` flag re-sends the active baseline with a `Full` frame the
  moment a `Feedback::Missing` arrives. `resend` is 0 everywhere: the scripted
  reliable lane retransmits the `Retire` itself, so the encoder's own retry is
  only needed for a link that loses answers outright — which the unit test below
  exercises directly.

## Invariants re-tested

The encoder and decoder contract in `udp_snapshot.rs` was re-exercised at every
lifetime this experiment enables (8, 16, 32, 64), on one synthetic source
schedule (512 frames):

| Test | Assertion | Result |
|---|---|---|
| `retirement_pins_two_baselines_until_ack_and_old_full_cannot_resurrect` | Two pinned baselines while a `Retire` is outstanding; no third proposal; an old `Full` cannot resurrect after `retired_through`; rotation resumes | pass ×4 |
| `a_lost_retired_answer_is_resent_and_rotation_resumes` | A decoder that retires but answers nothing still resumes rotation after `resend_retire`, with `pending` blocked until then | pass ×4 |
| `missing_baseline_recovers_and_epoch_change_rejects_old_traffic` | `Feedback::Missing` re-pins the baseline; an older `epoch` is refused on both directions | pass ×4 |
| `sustained_updates_save_encoded_bytes_and_caches_stay_bounded` | `deltas > 100`, `sent_bytes * 2 < full_bytes`, `retires > 0`, `baselines.len() <= 2` at all times | pass ×4 |
| `size_triggered_rotation_fires_within_the_lifetime_bound` | A 60% trigger on a 64-frame cap still proposes and retires | pass |
| `probe_instrument_is_alive` | Non-zero world updates, anchors, selected/independent bytes, fragments, `Full` and `Retire`; `selected < independent`; for all four workloads | pass |
| `probe_replays_byte_for_byte`, `probe_accounting_is_consistent`, `probe_is_workload_sensitive` | Determinism, the class-sum identity, workload sensitivity | pass |

Independent/delta encoded bytes at each lifetime on that synthetic schedule:
8 → 146987, 16 → 132309, 32 → 124891, 64 → 121084 (independent 350803), with
64/32/15/7 proposals and 63/31/15/7 retirements. That is the same shape as the
real sweep: from 32 to 8 buys delta bytes but pays four times the proposal
traffic.

## Honest limits

- **In-process, not the real binaries.** Everything is the production
  `PeerCodec`/`ClientCodec`/`Encoder`/`Pacer` and the production `Simulation`,
  driven by the test-only `bandwidth_probe` on a `ManualTime` clock. No socket,
  no GNS, no `HostPeer::pump`, no bounded peer outbox, no `Welcome`, no
  congestion control and no native header overhead. Application bytes only.
- **Synthetic workload.** A scripted pilot cadence and a bare field; 12 players
  is twelve world states on one wire with only the first commanded. No
  human-driven aims, no ramps, no robot contact, no join/leave.
- **One-off runs, not a 5-seed acceptance sweep.** The probe is deterministic,
  so repetition adds nothing, but there is no cross-machine or multi-seed
  variation here. The loss figures come from one seeded stream per profile.
- **The blackout measurement is coordinator-free.** `scripted_link` delays the
  reliable lane past a blackout but does not model a retransmit byte cost, so
  the loss table under-counts what a real reliable lane would spend. That is
  conservative for the decision: extra reliable traffic would not favour a
  shorter lifetime.
- **`loss_report`'s `stall` for the loss profiles is the nominal 3.000 s
  outage window, not a real outage.** Those rows have no blackout, so `stall`
  there is the ordinary inter-checkpoint gap; it is reported to show that loss
  alone never produces a 500 ms-class stall.
- **Not a playability result.** Fewer or more `Full` frames change correction
  points and replay cost, which this instrument does not measure. The
  recommendation is a bandwidth answer only.

## Recommendation

**Keep 32.** It is the current default and it is the best fixed lifetime for the
workloads that dominate the downstream budget: best of the four on `twelve`
(955.1 kbps, −2.4% against 64) and on `fire` (741.0 kbps, tied with 64), and
−6.8%/−4.6% against 8 and 16 there. 64 is 0.5–0.9% better on `idle`/`drive`
(+3.0/+4.9 kbps) but 22.6 kbps worse on `twelve`, so adopting it trades the
budget's largest consumer for the smallest. 8 and 16 lose everywhere; the
checkpoint does get smaller with a shorter lifetime, but each extra world
publication also carries an owner anchor, so the anchors grow faster than the
world frames shrink.

Do not adopt the size-triggered policy. Above the observed delta share it is
inert (70% never fires), below it is a rotation storm (55% on `fire`: +38%
bytes/s; 45%: +50%), and its best rows are identical to 64, which already loses
to 32 on the heavy workloads. Loss does not change the answer: every lifetime
delivered within 1.4–5.7% of its lossless byte total (1.008–1.014× at 1% loss,
1.037–1.043× at 5%, 1.057× under the blackout), `missing` was 0 in all
16 rows, and the stall after 1%/5% loss and a 0.5 s blackout was 32–64 ms /
544 ms at every lifetime. There is no loss-driven reason to move off 32, and no
lossless gain large enough to justify a change.
