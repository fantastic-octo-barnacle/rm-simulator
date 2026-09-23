<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Completed network investigation

NET-001 through NET-003 are closed as investigation items. The measurements here
belong to protocol 27–29 builds in September 2026. Closing this investigation
is not a claim that those rates or latencies describe protocol 52, or that the
historical 100–200 kbps target was measured as met. The original issue text and
trial interpretation remain in Git history; the [bandwidth experiments](bandwidth-experiments.md)
record the isolated changes and their revision IDs.

| Id | Historical finding | Recorded evidence |
|---|---|---|
| NET-001 | Downstream traffic exceeded the target in the measured workload | A one-client movement/firing trial measured about 103 kbps upstream and 855 kbps downstream in UDP proxy payloads (about 115/882 kbps with the stated header allowance). A separate five-seed, two-client matrix measured 1,255 kbps downstream per client; experimental cadence and DEFLATE changes lowered it to 662 kbps but failed constrained-link acceptance. DEFLATE was removed in protocol 35. |
| NET-002 | Shot outcomes were delayed on a fast local connection | The single-player smoke test measured 0.017 ms p95 command queue wait and about 51 ms p95 from submission to first outcome publication. A separate listen-host trial measured 5.95/9.78 ms median/p95 application RTT and 56.1/57.8 ms shot confirmation. These endpoints are different and exclude click-to-visible-hit latency. |
| NET-003 | Impaired links produced latency and incomplete local outcomes | Protocol 27 severe-impairment trials measured 1,732/2,590 ms median/p95 application RTT, 1,174/3,398 ms shot confirmation and 14 unresolved local outcomes. A matched short-blackout two-client trial recorded 6 unresolved outcomes at baseline `3a41fcc` and 7 at integrated build `5c4891a`; healthy clients had zero. A local unresolved outcome alone did not prove an authoritative hit was lost. |

The matched blackout trial used seed 2026, 1 s warmup, 12 s active and 3 s
recovery. One client had 35 ms per-direction delay, up to 15 ms jitter, 2% loss,
downstream reordering and duplication, and a 500 ms blackout starting at 3 s.
Its longest complete-checkpoint arrival interval was 593 ms at baseline and
590 ms on the integrated build; shot-confirmation p95 was 157 and 170 ms.
The impairment scenarios remain in `scripts/network-scenarios/`; raw local
captures were never permanent report URLs.

The historical tests did not measure cross-platform or twelve-real-client
performance, end-to-end click-to-visible-hit latency, or hit disagreement under
impairment. These limits describe the old evidence, not a pending validation
requirement for the completed investigation. For new measurements, use
[network tracing](network-tracing.md) and keep raw captures outside Git.
