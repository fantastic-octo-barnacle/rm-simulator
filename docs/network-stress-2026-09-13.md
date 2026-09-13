<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# UDP impairment and version compatibility tests

These tests follow the protocol 27 latency fixes. The network trials preceded
the lobby selection and connection error fixes described below. The new scenarios use real UDP datagrams through the local
proxy, with one healthy and one impaired rendered client sharing a headless host.
Each run has a 24-second active interval and eight seconds of recovery. Input
releases at 16 seconds, resumes firing at 20 seconds, and releases at 23 seconds.
The recovery phase releases inputs but retains the network profile. No compilation
or competing simulator trial overlapped a measured run.

The results and binary hashes are retained locally in
`/tmp/rm-network-pr-local-evidence/network-stress-2026-09-13/`, outside Git.
The measured builds included uncommitted changes; binary hashes distinguish them
from the Downloads release. Raw logs remain in the corresponding `/tmp/rm-*-20260913`
directories. Each timing distribution samples the latest reported state, rather
than recording every individual packet or launch.

## Delay, loss and bandwidth are different failures

| Profile for impaired client | Confirmed launches, healthy / impaired | Impaired launch confirmation p95 | Unresolved shots during active samples |
|---|---:|---:|---:|
| 35 ms + 0–15 ms jitter each way, 2% loss, ample bandwidth | 320 / 321 | 136 ms | 0 |
| Same delay/loss, 256 kbps up and 1,024 kbps down | 312 / 47 | 1,558 ms | 0 |
| 80 ms + 0–30 ms jitter, 8% loss, 256/768 kbps, two-second blackout | 267 / 50 | 3,398 ms | 14 |

The first two also introduce 2% downstream reordering with 30 ms extra delay and
1% duplication. The severe profile uses 5% reordering with 50 ms extra delay.
The blackout covers seconds 8–10 in both directions.

With ample bandwidth, the impaired client had no rejected or unresolved launches.
Its sampled application RTT p50/p95 was 96/200 ms, with transport RTT 74/78 ms.
The healthy peer remained near 6 ms application RTT. Full-context receipt gap
peaked at 54 ms, and the remote interpolation setting had a 131 ms median.

The bandwidth-limited run passed the existing liveness checks, but failed the
practical playability goal. The host offered about 186 kB/s of UDP payload to the
impaired downstream hop, whose nominal capacity was 128 kB/s before header costs.
The proxy dropped 1,529 downstream datagrams for queue wait and only 56 for random
loss during its active accounting window. Upstream also exceeded capacity and
lost 685 datagrams to queue wait. Application RTT reached a 1,036 ms median;
the remote buffer hit its 250 ms ceiling. Full-world context stopped arriving for
as long as 12.5 seconds even while owner anchors continued. There were 229 local
shot rejections. Recent owner updates therefore cannot stand in for world-state
health when assessing a connection.

The severe run failed the unresolved-shot gate and sample-count gates. Its three
console query gaps also delayed the shared sampling schedule, reducing samples
for the healthy peer despite zero healthy-client query gaps. By the end of the
eight-second recovery period the impaired client had confirmed 59 launches, had
15 unresolved outcomes in total, and had no pending shots. Full-context receipt
gap was about 158 ms and the high-latency warning remained. The proxy recorded
both explicit blackout loss and substantial additional capacity-related drops.

These runs justify working on bandwidth adaptation and compact state delivery.
Raising the application cap fixed loopback starvation, but it does not make a
low-capacity path carry a larger stream. Neither a survival gate nor fresh own
anchors prove usable enemy state. Hitscan rewind would not fix this failure.

## Downloads versus current build

The Downloads first release uses protocol 26; the current build uses protocol 27.
Both hosts in this matrix are the actual app binaries, running as rendered listen
hosts. All joins passed through an unimpaired UDP proxy. Matching-version pairs
were positive controls.

| Host | Client | Result |
|---|---|---|
| Downloads | Downloads | Joined |
| Current | Current | Joined |
| Downloads | Current | Rejected, no chassis allocated |
| Current | Downloads | Rejected, no chassis allocated |

Both mismatched connections failed in about 0.3–0.4 seconds from client process
launch. Both hosts stayed available and accepted a matching replacement client
without restarting. The client still answered console requests and could quit.
All test processes and proxy endpoints were closed afterward.

Before the error handling fix, the host log identified `incompatible protocol`,
but both clients reported:

```text
GNS hello failed: channel is empty and sending half is closed
```

The loading code uses that same startup error for the title-screen status. This
is a message propagation defect: the GNS worker records the disconnect reason
in the inbox, but the waiting constructor returns the welcome channel's closure
error instead. The rejection itself works; the player explanation does not.
The console also returns the stored startup failure for a later `state` query,
so that response is not evidence the app crashed.

The follow-up fix preserves the host's reason and gives guidance to update both
games. Current hosts include both protocol numbers in their rejection reason.
An unchanged Downloads client cannot gain that improved UI message through a
server update alone.

## Proposed bandwidth target

The user's 100–200 **kbps** means 12.5–25 kB/s, not 100–200 kB/s. A proposed first
target is 200 kbps downstream and 50–64 kbps upstream per client during gameplay,
including IP/UDP and native protocol overhead. Treat 100 kbps downstream as a
stretch target. Initial synchronization and reconnect recovery need separate,
bounded burst measurements. These are engineering targets, not demonstrated
limits for a full RoboMaster lobby.

At 30 updates/s, 200 kbps permits about 833 bytes per update before overhead.
The two-client workload currently uses roughly 170–190 kB/s downstream per peer,
about 1.4–1.5 Mbps. Simply setting a 200 kbps cap would reproduce the starvation
seen above more severely.

A relevant physics-networking reference is Glenn Fiedler's
[state synchronization article](https://gafferongames.com/post/state_synchronization/),
which works with a 256 kbps target and prioritizes updates within that budget.
The [snapshot compression article](https://www.gafferongames.com/post/snapshot_compression/)
shows why encoding only changed fields and packing them carefully matters.
Those examples support investigating this range; they do not establish this
simulator's bandwidth at its maximum player/projectile count.

Reaching the budget requires compact input and state encoding, frequent motion
and target updates separated from coherent restoration checkpoints, and explicit
service for launch/hit feedback. Checkpoint cadence and precision must be measured
against whole-field replay correctness. Preserve complete field restoration and
the 1 ms physics clock. Do not construct a reduced client physics world to meet
a byte target.

## Blackout without bandwidth saturation

A fourth run retains the ample-bandwidth delay/loss profile and adds a two-second
blackout. It fails the zero-unresolved-shot gate: 19 shot outcomes remain
unresolved, with 283 confirmed launches on the impaired peer and 319 on the
healthy peer. Both clients remain responsive and move more than eight metres.
The impaired checkpoint gap reaches 2,003 ms. This isolates a blackout recovery
issue from the bandwidth overload. The gate remains unchanged; recovery should
explicitly settle expired shot intents rather than silently lose their outcome.

## Lobby and error handling follow-up

Incompatible lobby rows are disabled, with the existing selection guard retained.
Direct UDP joins now preserve the worker's rejection reason instead of reporting
an internal channel closure. The current client translates the first release's
`incompatible protocol` reason into update guidance. Current hosts report both
protocol numbers over TCP and UDP. The unchanged Downloads client still has its
old error handling and cannot display this improvement until updated.

The rebuilt application passed all four combinations again. A current client
joining the Downloads host reports `Version mismatch. Update both games to the
same version.` Both mismatches allocate no robot, and each host accepts a matching
replacement client without restarting. The local evidence directory contains `version-matrix-fixed.json` and the
`fixed-` logs with binary hashes and post-fix evidence.

Recorded artifact paths use `<checkout>`, `<downloads>` and `<home>` placeholders
for local directories. Measurements and binary hashes are unchanged.
