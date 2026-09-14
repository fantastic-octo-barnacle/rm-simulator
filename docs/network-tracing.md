<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Network tracing

Packet metadata, capture commands and local channel diagnostics for the
gameplay transport. The game keeps cumulative metadata counters even with disk
tracing disabled. Console `state` exposes them under `network.trace`. The
detailed network overlay shows whether recording is off, active, capped or
failed. Reading diagnostics never requests a host snapshot or waits for a
transport worker.

## Contents

- [Enable recording](#enable-recording)
- [Trace files and event records](#trace-files-and-event-records)
- [Stages](#stages)
- [Measurement semantics](#measurement-semantics)
- [Byte counters](#byte-counters)
- [Bounds and incomplete recordings](#bounds-and-incomplete-recordings)
- [Summary tool](#summary-tool)
- [Embedded play](#embedded-play)
- [Packet classification](#packet-classification)
- [Deterministic bandwidth probe](#deterministic-bandwidth-probe)

## Enable recording

Enable recording before launching each process:

```sh
RM_NET_TRACE_DIR=/tmp/rm-network-traces just run --network-stats detailed
RM_NET_TRACE_DIR=/tmp/rm-network-traces just server
```

Use a separate terminal for the server when testing remote clients. Environment
settings apply only to that process and its children. Filenames include the
process id, role and a unique suffix, and existing files are never overwritten.
The simulation worker, UDP host reactor and each client/ TCP writer have separate
files. Local player files use the `local` role. The environment variable is read
when an observer is constructed, so restart the session/process to change it.

## Trace files and event records

Each JSONL file starts with a schema/protocol header. Events record observer-local
elapsed nanoseconds, stage, class and relevant snapshot, fragment, input, shooter,
shot, projectile or hit identifiers. Host command observations include the host
simulation tick time; input events include intended simulation time. No payloads,
player names, addresses, passwords or rejection text are recorded. File paths
appear in local console diagnostics only.

## Stages

| Stage | Meaning |
|---|---|
| `enqueue_attempt` / `dequeue` | Client submission attempt and transport worker consumption; a failed attempt need not have a dequeue |
| `submit_native` | Application datagram offered to GNS, including packets GNS may discard |
| `discard_native` | GNS explicitly ignored an unreliable submission |
| `receive` | Application datagram delivered by GNS, before our framing/decoding |
| `host_receive` / `host_accept` / `host_reject` | Host worker receipt and command validation; accepting a scheduled shot is not execution |
| `host_publish` | Logical snapshot, event or response produced by the host |
| `publish` / `consume` | Decoded client inbox publication and newest snapshot consumption by the session |
| `tcp_write` / `tcp_bytes` | TCP host message and its JSON-line bytes written |
| `work` | Measured transport work duration, including encoding, decoding or typed local publication |

## Measurement semantics

A host decode/submit measurement includes waiting to submit to the host mailbox.
Host encode/pace includes the whole pump, not just compression. Local publication
includes cloning the typed message and updating the inbox. These are measured
CPU-side elapsed durations, not GPU timings or packet transit times.

Application byte counters exclude GNS framing, encryption, retransmissions and
IP/UDP headers. `submit_native` is offered bytes, not proof of wire delivery.
GNS provider rates remain separate in `network.stats.native`. Do not sum stages:
the same bytes can appear at submission and receipt. Typed local messages have
`bytes: null` in counters because they have no wire representation; their event
counts and publication work are the useful measurements.

## Byte counters

Host delivery reports expose `encoding` counters once per second:

- `raw_world_bytes` is checkpoint JSON before delta compression, measured on the
  normal delta path; the full-checkpoint comparison path leaves this unmeasured.
- `framed_world_bytes` and `owner_bytes` count produced application bytes before
  pacing replaces or expires them.
- `independent_bytes` and `selected_bytes` compare the baseline codec's compressed
  full alternative with its selected full/delta encoding, before fragment headers.
- `skipped_world_updates` counts world updates skipped for native backlog.

Existing `sent_bytes`, `queued_bytes` and `oldest_age_ms` in the delivery report
measure pacer service/backlog by control, owner and world class. Production,
pacing and native delivery are different stages; preserve that distinction when
comparing an optimization. Older hosts omit `encoding` rather than reporting zero.

## Bounds and incomplete recordings

Each observer has a 2,048-event writer queue and a 64 MiB file cap. Producers use
nonblocking submission; full queues increase `dropped` while gameplay continues.
Counter-lock contention skips an observation and increases `contended`; frame
updates never wait for logging. File failures disable disk recording while
counters continue. Record both loss counters when judging a trace.

The writer flushes every 128 events or after 250 ms without an event. Observer shutdown
closes the queue and joins the writer after it drains and writes a terminal status. Abrupt
process termination can leave an incomplete final line or no terminal status.
The summary tool reports this instead of treating the file as complete. At the
size cap the writer stops recording new events; console counters still update.

## Summary tool

```sh
python3 scripts/network-trace.py /tmp/rm-network-traces/*.jsonl > /tmp/rm-network-summary.json
```

The tool keeps files separate because their clocks have different origins. It
pairs client enqueue/dequeue and first shot outcome events within each file,
retaining the latest 10,000 timing pairs and at most 4,096 outstanding identities.
Missing events are not inferred rejections or successful shots. Both accepted and
rejected terminal outcomes count toward the shot-outcome timing distribution.

Keep raw traces and generated summaries outside Git. Compare trace-disabled and
trace-enabled runs when measuring performance; opt-in recording still consumes
CPU and disk bandwidth even though it cannot block gameplay on disk I/O.

## Embedded play

`Server::connect_owner` uses bounded typed command and snapshot channels. It
retains the same `Client` API, host authorization and confirmation snapshot/Pong
order as network play. Only unsent periodic snapshots may be replaced. Reliable
outbox overflow closes the peer explicitly. Dropping the client removes its seat;
server teardown joins its delivery workers. Standalone sessions create no gameplay
listener, while listen hosts use channels for their local player and GNS/TCP for
remote players. Snapshot cloning and physics replay still cost time.

## Packet classification

Packet classification recognizes both RMO3/RMO4 owner anchors and RMI2/RMI3
input batches. Before `e678491`, the protocol 29 tags were counted as `control`;
that affects historical per-class attribution, not total byte counts.

## Deterministic bandwidth probe

Run `cargo test -p rm-simulator-server --locked bandwidth_attribution_baseline
-- --nocapture` on one line to measure the production peer/client codecs on a
manual clock. The probe reports owner/world/control bytes, input batches and
complete checkpoints for idle, driving, firing and twelve-chassis workloads.
It accounts for the current RMO4/RMI3 wire formats. Use a separate target directory
per concurrent worktree to avoid executing another checkout's test artifact.

These application-byte measurements exclude GNS and network overhead. The probe
is not a real-UDP acceptance trial, and its canonical firing scenarios do not
populate shot-result or hit histories. See the [experiment record](bandwidth-experiments.md)
for historical comparisons and outstanding workload coverage.
