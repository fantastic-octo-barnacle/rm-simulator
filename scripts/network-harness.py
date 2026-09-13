#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Run real simulator clients through isolated UDP impairments and retain a report.

Uses existing binaries and console diagnostics. Does not build or change game networking.
"""
import argparse
import collections
import concurrent.futures
import dataclasses
import hashlib
import json
import math
import os
from pathlib import Path
import re
import platform
import signal
import socket
import subprocess
import sys
import threading
import time
import urllib.request

from network_harness_lib import Profile, ProxyFarm, load_json, number

ROOT = Path(__file__).resolve().parents[1]
MAX_LINE = 4 * 1024 * 1024
MAX_ARTIFACT = 64 * 1024 * 1024
# A sample whose console reply does not arrive within the console timeout is a gap,
# not a dead trial; only a console that keeps failing ends the run.
MAX_CONSECUTIVE_SAMPLE_FAILURES = 5
CONSOLE_TIMEOUT_S = 2
DEFAULT_BASELINE_TOLERANCE = .25
# Every summarised distribution statistic is "worse when larger", byte rates
# included: a run that needs more bandwidth for the same workload has regressed,
# and a run that needs less has not.
OPERATORS = {'==': lambda observed, threshold: observed == threshold,
             '!=': lambda observed, threshold: observed != threshold,
             '<': lambda observed, threshold: observed < threshold,
             '<=': lambda observed, threshold: observed <= threshold,
             '>': lambda observed, threshold: observed > threshold,
             '>=': lambda observed, threshold: observed >= threshold}
METRIC_PATTERN = re.compile(r'[a-z][a-z0-9_]*(\.[a-z0-9_]+){0,3}')
DISTRIBUTION_STATS = ('p50', 'p95', 'p99', 'max')


def validate_expectations(data, client_names):
    """Normalise the optional expectations block; raise ValueError on anything unknown."""
    if not isinstance(data, dict) or set(data) - {'baseline_tolerance', 'checks'}:
        raise ValueError('expectations accept baseline_tolerance and checks only')
    tolerance = number(data.get('baseline_tolerance', DEFAULT_BASELINE_TOLERANCE),
                       'baseline_tolerance', 0, 100)
    checks = data.get('checks', [])
    if not isinstance(checks, list) or not 1 <= len(checks) <= 64:
        raise ValueError('expectations need 1 to 64 checks')
    seen, normalized = set(), []
    for check in checks:
        if not isinstance(check, dict) or set(check) - {
                'name', 'metric', 'op', 'value', 'per_client', 'client', 'on_missing'}:
            raise ValueError('invalid expectation fields')
        name = check.get('name')
        if not isinstance(name, str) or not re.fullmatch(r'[a-z0-9_]{1,64}', name) or name in seen:
            raise ValueError('expectation names must be unique lowercase identifiers')
        seen.add(name)
        metric = check.get('metric')
        if not isinstance(metric, str) or not METRIC_PATTERN.fullmatch(metric):
            raise ValueError('expectation metric must be a dotted summary path')
        if check.get('op') not in OPERATORS:
            raise ValueError(f'expectation operator must be one of {sorted(OPERATORS)}')
        number(check.get('value'), 'expectation value', -(2**53), 2**53)
        per_client = check.get('per_client', False)
        if not isinstance(per_client, bool):
            raise ValueError('per_client must be a boolean')
        client = check.get('client')
        if client is not None and (not isinstance(client, str) or client not in client_names):
            raise ValueError('expectation names an unknown client')
        if per_client and client is not None:
            raise ValueError('a check selects every client or one client, not both')
        on_missing = check.get('on_missing', 'fail')
        if on_missing not in ('fail', 'skip'):
            raise ValueError('on_missing must be fail or skip')
        normalized.append({'name': name, 'metric': metric, 'op': check['op'],
                           'value': check['value'], 'per_client': per_client,
                           'client': client, 'on_missing': on_missing})
    return {'baseline_tolerance': tolerance, 'checks': normalized}


def validate_scenario(data):
    allowed = {'schema_version', 'name', 'seed', 'warmup_s', 'duration_s', 'recovery_s',
               'sample_hz', 'clients', 'events', 'shared_downstream', 'expectations'}
    if not isinstance(data, dict) or set(data) - allowed:
        raise ValueError('invalid or unknown scenario fields')
    if data.get('schema_version') != 1:
        raise ValueError('scenario schema_version must be 1')
    if not isinstance(data.get('name'), str) or not re.fullmatch(r'[a-z0-9_-]{1,64}', data['name']):
        raise ValueError('scenario name must be a short lowercase identifier')
    number(data.get('seed', 2026), 'seed', maximum=2**53)
    if not isinstance(data.get('seed', 2026), int):
        raise ValueError('seed must be an integer')
    for name, default, lower, upper in (('warmup_s', 10, 0, 300), ('duration_s', 60, 0.1, 3600),
                                      ('recovery_s', 10, 0, 300), ('sample_hz', 4, 0.1, 10)):
        number(data.get(name, default), name, lower, upper)
    clients = data.get('clients', [])
    if not isinstance(clients, list) or not 1 <= len(clients) <= 12:
        raise ValueError('scenario needs 1 to 12 clients')
    names = set()
    for client in clients:
        if not isinstance(client, dict) or set(client) - {'name', 'team', 'upstream', 'downstream'}:
            raise ValueError('invalid client fields')
        name = client.get('name', '')
        if not isinstance(name, str) or not re.fullmatch(r'[a-z0-9_-]{1,32}', name) or name in names or name == 'server':
            raise ValueError('client names must be unique lowercase identifiers')
        names.add(name)
        if client.get('team', 'red') not in ('red', 'blue'):
            raise ValueError('client team must be red or blue')
        for direction in ('upstream', 'downstream'):
            Profile.parse(client.get(direction, {}))
    if data.get('shared_downstream'):
        # The shared hop models only server bandwidth and queues, not a second loss path.
        shared = data['shared_downstream']
        if not isinstance(shared, dict) or set(shared) - {'rate_kbps', 'queue_bytes', 'queue_wait_ms', 'rate_changes'}:
            raise ValueError('shared_downstream supports rate and queue settings only')
        Profile.parse(shared)
    previous = -1
    events = data.get('events', [])
    if not isinstance(events, list) or len(events) > 10000:
        raise ValueError('events must be a list with at most 10000 entries')
    for event in events:
        if not isinstance(event, dict) or set(event) != {'at_s', 'client', 'command'}:
            raise ValueError('events require at_s, client and command')
        at = number(event['at_s'], 'event time', maximum=data.get('duration_s', 60))
        if at < previous:
            raise ValueError('events must be ordered by at_s')
        previous = at
        if not isinstance(event['client'], str) or event['client'] not in names:
            raise ValueError('event names an unknown client')
        command = event['command']
        if not isinstance(command, dict) or command.get('type') not in {
            'capture', 'key', 'mouse_button', 'mouse_motion', 'release_inputs', 'inspect',
            'camera', 'spawn', 'pause',
        }:
            raise ValueError('unsupported scenario command')
    expectations = validate_expectations(data['expectations'], names) if 'expectations' in data else None
    result = {**data, 'seed': data.get('seed', 2026), 'warmup_s': data.get('warmup_s', 10),
              'duration_s': data.get('duration_s', 60), 'recovery_s': data.get('recovery_s', 10),
              'sample_hz': data.get('sample_hz', 4), 'events': events}
    if expectations is None:
        result.pop('expectations', None)
    else:
        result['expectations'] = expectations
    return result


class JsonLog:
    def __init__(self, path):
        self.stream = path.open('x', encoding='utf8')
        self.bytes = 0
        self.dropped = 0
        self.lock = threading.Lock()

    def write(self, record):
        line = json.dumps(record, allow_nan=False, separators=(',', ':')) + '\n'
        with self.lock:
            if self.bytes + len(line.encode()) > MAX_ARTIFACT:
                self.dropped += 1
                return
            self.stream.write(line)
            self.stream.flush()
            self.bytes += len(line.encode())

    def close(self):
        self.stream.close()


class OwnedProcess:
    def __init__(self, name, command, output):
        self.name = name
        self.command = command
        self.dropped_bytes = 0
        self.cleanup = None
        self.log = (output / f'{name}.log').open('xb')
        try:
            self.process = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
                                            stderr=subprocess.STDOUT, start_new_session=True)
        except BaseException:
            self.log.close()
            raise
        self.thread = threading.Thread(target=self.drain, name=f'{name}-log', daemon=True)
        self.thread.start()

    def drain(self):
        written = 0
        while chunk := self.process.stdout.read(8192):
            keep = min(len(chunk), MAX_ARTIFACT - written)
            self.log.write(chunk[:keep])
            self.log.flush()
            written += keep
            self.dropped_bytes += len(chunk) - keep

    def stop(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=3)
                self.cleanup = 'terminated'
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=3)
                self.cleanup = 'killed_after_timeout'
        else:
            self.cleanup = 'already_exited'
        self.thread.join(timeout=3)
        self.process.stdout.close()
        self.log.close()


class ConsoleTimeout(RuntimeError):
    """The client did not answer within the console timeout; the trial records a gap."""


class Console:
    """One console connection. Reads are framed here rather than through a buffered
    file object, because a timed-out `socket.makefile` reader refuses every later read."""

    def __init__(self, port, timeout=CONSOLE_TIMEOUT_S):
        self.socket = socket.create_connection(('127.0.0.1', port), timeout=timeout)
        self.buffer = bytearray()
        self.nonce = 0

    def line(self):
        while True:
            index = self.buffer.find(b'\n')
            if index >= 0:
                line = bytes(self.buffer[:index])
                del self.buffer[:index + 1]
                return line
            if len(self.buffer) > MAX_LINE:
                raise RuntimeError('console response exceeded limit')
            chunk = self.socket.recv(65536)
            if not chunk:
                raise RuntimeError('console disconnected')
            self.buffer += chunk

    def command(self, command):
        self.nonce += 1
        request = (json.dumps({'id': self.nonce, 'command': command}) + '\n').encode()
        try:
            self.socket.sendall(request)
        except socket.timeout as exc:
            raise ConsoleTimeout(f'console did not accept {command.get("type")} in time') from exc
        while True:
            try:
                line = self.line()
            except socket.timeout as exc:
                raise ConsoleTimeout(f'console did not answer {command.get("type")} in time') from exc
            response = json.loads(line)
            identifier = response.get('id')
            # A reply to an earlier command that timed out: discard it and keep reading,
            # so one slow frame does not desynchronise every later command.
            if isinstance(identifier, int) and not isinstance(identifier, bool) and identifier < self.nonce:
                continue
            if identifier != self.nonce or not response.get('ok'):
                raise RuntimeError(f'console command failed: {response}')
            return response['result']

    def close(self):
        self.socket.close()


def free_port(kind):
    with socket.socket(socket.AF_INET, kind) as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def digest(path):
    hasher = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            hasher.update(chunk)
    return hasher.hexdigest()


def git_output(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT, text=True).strip()


def distribution(values):
    values = sorted(value for value in values if isinstance(value, (int, float)))
    if not values:
        return {'count': 0, 'p50': None, 'p95': None, 'p99': None, 'max': None}
    def percentile(p):
        return values[min(len(values) - 1, max(0, math.ceil(p * len(values)) - 1))]
    return {'count': len(values), 'p50': percentile(.5),
            'p95': percentile(.95) if len(values) >= 20 else None,
            'p99': percentile(.99) if len(values) >= 100 else None, 'max': values[-1]}


# Distributions collected straight from the console's `state.network` subtree.
NETWORK_METRICS = ('checkpoint_gap_ms', 'prediction_backlog_ms', 'input_lead_ms',
                   'correction_position_m', 'input_execution_lateness_ms',
                   'correction_orientation_rad', 'correction_velocity_m_s', 'correction_aim_rad',
                   'remote_view_age_ms', 'remote_buffer_delay_ms', 'remote_underruns',
                   'collision_context_age_ms', 'collision_context_gap_ms',
                   'snapshot_clock_offset_ms')
# Event distributions never fall back to the last-value console gauges.
EVENT_METRICS = ('rtt_ms', 'shot_confirmation_ms', 'shot_execution_offset_ms',
                 'checkpoint_arrival_interval_ms')


def event_distribution(rows, metric):
    """Count event IDs once over the first-to-last active observation window.

    The first available history establishes a cursor, excluding setup/warmup.
    Missing histories do not reset it. Gaps report overwritten events rather
    than silently claiming complete coverage; equal-valued events remain distinct.
    """
    cursor, values, missed = None, [], 0
    for row in rows:
        series = ((row['state'].get('network') or {}).get('event_samples') or {}).get(metric)
        if series is None:
            continue
        total = series['total']
        if cursor is None:
            cursor = total
            continue
        if total < cursor:
            raise ValueError('event sample counter reset during a client session')
        fresh = [(identity, value) for identity, value in series['values'] if identity > cursor]
        for identity, value in fresh:
            missed += max(0, identity - cursor - 1)
            values.append(value)
            cursor = identity
        missed += max(0, total - cursor)
        cursor = total
    return distribution(values), missed


# `state.network.stats.native`, absent unless the native transport reports.
NATIVE_METRICS = ('transport_rtt_ms', 'send_bytes_per_s', 'receive_bytes_per_s', 'send_queue_ms')
# `state.network.stats`; the provider may leave loss_percent null with a reason.
STATS_METRICS = {'transport_loss_percent': 'loss_percent'}
# Top-level console state fields.
STATE_METRICS = {'pending_shots': 'pending_shots'}
# Cumulative counters reduced to first-to-last deltas.
NETWORK_COUNTERS = {'confirmed_launches_delta': 'confirmed_launches',
                    'local_rejections_delta': 'rejections',
                    'unresolved_shot_outcomes_delta': 'unresolved_shot_outcomes',
                    'remote_underruns_delta': 'remote_underruns',
                    'prediction_results_discarded_delta': 'prediction_results_discarded',
                    'prediction_held_moving_frames_delta': 'prediction_held_moving_frames'}
# Global authoritative counters, observed through each client's own snapshots.
STATE_COUNTERS = {'observed_global_shots_fired_delta': 'shots_fired',
                  'observed_global_hits_detected_delta': 'hits_detected'}
# Report labels resolved from what the run actually collected.
AVAILABILITY = {'same_tick_correction_error': ('correction_position_m',),
                'input_execution_delay': ('input_execution_lateness_ms',),
                'per_shot_execution_and_confirmation_delay': ('shot_confirmation_ms',
                                                              'shot_execution_offset_ms'),
                'transport_rtt': ('transport_rtt_ms',),
                'transport_loss': ('transport_loss_percent',),
                'remote_presentation': ('remote_view_age_ms', 'remote_buffer_delay_ms'),
                'prediction_backlog': ('prediction_backlog_ms',),
                'collision_context_age': ('collision_context_age_ms',)}
# No source publishes these at all; they stay unavailable regardless of the run.
NEVER_COLLECTED = ('hit_disagreement', 'input_to_photon_latency',
                   'per_event_input_trace')


def summarize(samples, proxies, duration, gaps=(), schedule_skips=0):
    gaps = list(gaps)
    clients = {}
    names = sorted({sample['client'] for sample in samples} | {gap['client'] for gap in gaps})
    for name in names:
        rows = [sample for sample in samples if sample['client'] == name and sample['phase'] == 'active']
        def network(row):
            return row['state'].get('network') or {}
        metrics = {key: distribution([network(row).get(key) for row in rows]) for key in NETWORK_METRICS}
        event_losses = {}
        for key in EVENT_METRICS:
            metrics[key], event_losses[key] = event_distribution(rows, key)
        for key in NATIVE_METRICS:
            metrics[key] = distribution([((network(row).get('stats') or {}).get('native') or {}).get(key)
                                         for row in rows])
        for key, source in STATS_METRICS.items():
            metrics[key] = distribution([(network(row).get('stats') or {}).get(source) for row in rows])
        for key, source in STATE_METRICS.items():
            metrics[key] = distribution([row['state'].get(source) for row in rows])
        def delta(values):
            return values[-1] - values[0] if len(values) >= 2 and all(v is not None for v in values) else None
        coherent = [network(row).get('collision_context_coherent') for row in rows]
        coherent = [bool(value) for value in coherent if isinstance(value, bool)]
        positions = [(row['state'].get('chassis') or {}).get('pose', {}).get('translation_m') for row in rows]
        positions = [p for p in positions if p is not None]
        movement = max((sum((a-b)**2 for a, b in zip(p, positions[0]))**0.5 for p in positions), default=None)
        clients[name] = {'active_samples': len(rows), 'metrics': metrics,
                         'event_samples_missed': event_losses,
                         'max_displacement_m': movement,
                         'console_query_ms': distribution([(row['received_s'] - row['elapsed_s']) * 1000
                                                            for row in rows]),
                         'sample_gaps': sum(1 for gap in gaps if gap['client'] == name),
                         'collision_context_coherent_fraction':
                             sum(coherent) / len(coherent) if coherent else None,
                         **{key: delta([network(row).get(source) for row in rows])
                            for key, source in NETWORK_COUNTERS.items()},
                         **{key: delta([row['state'].get(source) for row in rows])
                            for key, source in STATE_COUNTERS.items()},
                         'warnings': dict(collections.Counter(
                             row['state'].get('connection_toast') or 'none' for row in rows))}
    active_proxies = [row for row in proxies if 0 <= row['elapsed_s'] <= duration]
    rates = {}
    if len(active_proxies) >= 2:
        first, last = active_proxies[0], active_proxies[-1]
        elapsed = last['elapsed_s'] - first['elapsed_s']
        for name, peer in last['peers'].items():
            rates[name] = {}
            for direction, counters in peer.items():
                old = first['peers'][name][direction]
                rates[name][direction] = {
                    'window_s': elapsed,
                    **{key + '_per_s': (counters[key] - old[key]) / elapsed
                       for key in ('received_bytes', 'forwarded_bytes', 'serviced_accounted_bytes')},
                    'dropped_delta': counters['dropped'] - old['dropped'],
                    'drop_reasons_delta': {reason: value - old.get('drop_reasons', {}).get(reason, 0)
                                          for reason, value in counters.get('drop_reasons', {}).items()},
                }
    def collected(keys):
        return any(client['metrics'][key]['count'] for client in clients.values() for key in keys)
    return {'event_metric_schema': 1, 'clients': clients, 'proxy_rates': rates,
            'sample_gap_total': len(gaps),
            'sample_schedule_skips': schedule_skips,
            'sample_gap_detail': gaps[-64:],
            'total_downstream_received_bytes_per_s': sum(
                peer.get('downstream', {}).get('received_bytes_per_s', 0) for peer in rates.values()) if rates else None,
            'proxy_final': proxies[-1] if proxies else None,
            'unavailable': sorted(label for label, keys in AVAILABILITY.items() if not collected(keys))
                           + list(NEVER_COLLECTED),
            'notes': ['rtt_ms counts individual reliable application probes, not repeated console readings.',
                      'Event distributions cover first-to-last available active histories; missing IDs are reported.',
                      'checkpoint_gap_ms and collision_context_gap_ms are sampled receive ages, not arrival intervals.',
                      'shots_fired is global authoritative state, not per-player shot acceptance;'
                      " confirmed_launches counts this client's accepted launches.",
                      'Shot deltas span first to last sampled value, not the entire active interval.',
                      'Rates use actual proxy timestamps; received/forwarded bytes exclude IP/UDP headers.',
                      'Correction and execution metrics require protocol 18; older builds produce null values.',
                      'Execution telemetry is a sampled latest transition, not a complete input event trace.',
                      'A sample gap is a console query that timed out; its metrics are absent, not zero.',
                      'No percentile is inferred from missing measurements.']}


def lookup(container, path):
    """Walk a dotted path through nested dicts. Returns (value, found)."""
    node = container
    for part in path.split('.'):
        if not isinstance(node, dict) or part not in node:
            return None, False
        node = node[part]
    return node, True


def client_lookup(client, path):
    """Client metrics accept both `metrics.rtt_ms.p95` and the `rtt_ms.p95` shorthand."""
    value, found = lookup(client, path)
    if found:
        return value, True
    return lookup(client.get('metrics', {}), path)


def evaluate_checks(report, expectations):
    """Apply each configured check, producing one row per evaluated target."""
    rows = []
    for check in expectations.get('checks', ()):
        if check.get('per_client'):
            # A per-client check over no clients must not pass by vacuum.
            targets = [(name, client_lookup, client) for name, client in report['clients'].items()] \
                or [(None, None, None)]
        elif check.get('client') is not None:
            client = report['clients'].get(check['client'])
            targets = [(check['client'], client_lookup, client)] if client is not None else \
                      [(check['client'], None, None)]
        else:
            targets = [(None, lookup, report)]
        for name, resolve, container in targets:
            row = {'name': check['name'], 'client': name, 'metric': check['metric'],
                   'operator': check['op'], 'threshold': check['value'], 'observed': None}
            if container is None or resolve is None:
                value, found = None, False
            else:
                value, found = resolve(container, check['metric'])
            if not found or value is None:
                row['status'] = 'skipped' if check.get('on_missing') == 'skip' else 'failed'
                row['detail'] = ('no client was sampled' if container is None
                                 else 'metric absent from this run')
            elif not isinstance(value, (int, float)):
                row['status'] = 'failed'
                row['detail'] = f'metric is not numeric: {type(value).__name__}'
            else:
                row['observed'] = value
                row['status'] = 'passed' if OPERATORS[check['op']](value, check['value']) else 'failed'
                row['detail'] = f'{value} {check["op"]} {check["value"]}'
            rows.append(row)
    return rows


def compare_baseline(report, baseline, tolerance):
    """Fail any summarised distribution statistic that grew by more than the tolerance.

    Larger is worse for every statistic the harness keeps, byte rates included.
    """
    rows, regressions = [], 0
    for name, client in sorted(report['clients'].items()):
        reference = (baseline.get('clients') or {}).get(name)
        for metric, values in sorted(client.get('metrics', {}).items()):
            for stat in DISTRIBUTION_STATS:
                path = f'{metric}.{stat}'
                new = values.get(stat)
                old = ((reference or {}).get('metrics') or {}).get(metric, {}).get(stat)
                row = {'client': name, 'metric': path, 'baseline': old, 'observed': new}
                if metric in EVENT_METRICS and report.get('event_metric_schema') != baseline.get('event_metric_schema'):
                    row['status'] = 'skipped'
                    row['detail'] = 'incompatible event measurement schema'
                elif old is None or new is None:
                    row['status'] = 'skipped'
                    row['detail'] = 'missing in baseline' if old is None else 'missing in run'
                elif metric in ('checkpoint_gap_ms', 'collision_context_gap_ms') and stat != 'max':
                    row['status'] = 'skipped'
                    row['detail'] = 'sample-phase-dependent receive age; compare arrival intervals instead'
                else:
                    limit = old * (1 + tolerance)
                    row['limit'] = limit
                    row['status'] = 'failed' if new > limit + 1e-9 else 'passed'
                    row['detail'] = ('regressed beyond tolerance' if row['status'] == 'failed'
                                     else 'within tolerance')
                    regressions += row['status'] == 'failed'
                rows.append(row)
    return {'tolerance': tolerance, 'regressions': regressions,
            'compared': sum(1 for row in rows if row['status'] != 'skipped'),
            'skipped': sum(1 for row in rows if row['status'] == 'skipped'),
            'comparisons': rows[:4096]}


def run(args, scenario):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    logs = {name: JsonLog(output / (name + '.jsonl')) for name in ('samples', 'events', 'proxy')}
    processes, consoles = [], {}
    samples, proxies = collections.deque(maxlen=10000), collections.deque(maxlen=4096)
    gaps = collections.deque(maxlen=4096)
    consecutive_failures = collections.Counter()
    sample_count, schedule_skips = 0, 0
    farm = None
    status, error = 'failed', None
    stage = 'setup'
    started = time.monotonic()
    try:
        manifest = {'schema_version': 1, 'scenario': scenario, 'manual': args.manual,
                    'effective_profiles': {client['name']: {direction: dataclasses.asdict(Profile.parse(client.get(direction, {})))
                                          for direction in ('upstream', 'downstream')} for client in scenario['clients']},
                    'harness_sha256': {name: digest(ROOT / 'scripts' / name)
                                      for name in ('network-harness.py', 'network_harness_lib.py')},
                    'commit': git_output('rev-parse', 'HEAD'), 'dirty': git_output('status', '--porcelain'),
                    'platform': platform.platform(), 'machine': platform.machine(),
                    'environment': {key: os.environ[key] for key in
                                    ('WGPU_SETTINGS_PRIO', 'WGPU_BACKEND', 'WGPU_POWER_PREF', 'RUST_LOG',
                                     'RM_NET_PROFILE', 'RM_NET_UP_KIB_S', 'RM_NET_DOWN_KIB_S', 'RM_NET_FULL_CHECKPOINTS', 'RM_NET_FIXED_INPUT_LEAD', 'RM_NET_INPUT_HISTORY')
                                    if key in os.environ},
                    'python': sys.version, 'protocol_source': (ROOT / 'crates/rm-simulator-server/src/protocol.rs').read_text().split('pub const PROTOCOL_VERSION: u32 = ')[-1].split(';')[0],
                    'binaries': {name: {'path': str(path), 'sha256': digest(path)}
                                 for name, path in (('server', args.server), ('app', args.app))},
                    'cad': {str(path.relative_to(args.cad_assets)): digest(path)
                            for path in (args.cad_assets / 'manifest.json', args.cad_assets / 'equipment/manifest.json')},
                    'timing': 'Setup and warmup are unimpeded; impairment time zero is active workload start. Recovery releases controls but retains the configured link.',
                    'accounting': 'Rates charge UDP payload plus profile overhead bytes, default 28 IPv4/UDP bytes. Shared egress is a separate serial bottleneck before each downstream link.',
                    'limits': {'artifact_bytes_per_file': MAX_ARTIFACT, 'console_response_bytes': MAX_LINE},
                    'build_profile': args.build_label}
        (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        server_port, http_port = free_port(socket.SOCK_DGRAM), free_port(socket.SOCK_STREAM)
        server = OwnedProcess('server', [str(args.server), '--listen', f'127.0.0.1:{server_port}',
            '--http', f'127.0.0.1:{http_port}', '--cad-assets', str(args.cad_assets)], output)
        processes.append(server)
        # This endpoint belongs to the loopback child we just spawned. Never
        # send its readiness probe through an environment or macOS system proxy.
        local_http = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        deadline = time.monotonic() + args.ready_timeout
        while True:
            if server.process.poll() is not None:
                raise RuntimeError('server exited during startup; see server.log')
            try:
                with local_http.open(f'http://127.0.0.1:{http_port}/api/state', timeout=1) as response:
                    if response.status == 200:
                        break
            except (OSError, ValueError) as exc:
                if time.monotonic() >= deadline:
                    raise RuntimeError(f'server readiness timed out: {exc}') from exc
                time.sleep(.1)
        def proxy_report(row):
            logs['proxy'].write(row)
            proxies.append(row)
        specs = [{**client, 'target_port': server_port} for client in scenario['clients']]
        farm = ProxyFarm(specs, seed=scenario['seed'], shared_downstream=scenario.get('shared_downstream'),
                         report=proxy_report, armed=False).start()
        ports = {}
        for client, proxy_port in zip(scenario['clients'], farm.ports):
            port = free_port(socket.SOCK_STREAM)
            ports[client['name']] = port
            command = [str(args.app), '--connect', f'127.0.0.1:{proxy_port}', '--console', f'127.0.0.1:{port}',
                       '--name', 'harness-' + client['name'], '--team', client.get('team', 'red'),
                       '--cad-assets', str(args.cad_assets)]
            if getattr(args, 'network_stats', None):
                command += ['--network-stats', args.network_stats]
            if not args.manual or client is not scenario['clients'][0]:
                command += ['--window-mode', 'headless']
            processes.append(OwnedProcess(client['name'], command, output))
        deadline = time.monotonic() + args.ready_timeout
        while len(consoles) < len(ports):
            for name, port in ports.items():
                if name in consoles:
                    continue
                console = None
                try:
                    console = Console(port, args.console_timeout)
                    state = console.command({'type': 'state'})
                    if state.get('ready'):
                        consoles[name] = console
                        console = None
                except (OSError, ValueError, RuntimeError):
                    pass
                finally:
                    if console:
                        console.close()
            if any(p.process.poll() is not None for p in processes):
                raise RuntimeError('process exited before all clients were ready')
            if time.monotonic() >= deadline:
                raise RuntimeError('client readiness timed out')
            time.sleep(.1)
        manifest['processes'] = [{'name': p.name, 'pid': p.process.pid, 'command': p.command} for p in processes]
        manifest['ports'] = {'server': server_port, 'http': http_port, 'console': ports, 'proxy': farm.ports}
        (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        print(f'All clients ready. Artifacts: {output}', flush=True)
        stage = 'warmup'
        deadline = time.monotonic() + scenario['warmup_s']
        while time.monotonic() < deadline:
            if any(p.process.poll() is not None for p in processes):
                raise RuntimeError('process exited during warmup')
            time.sleep(min(.1, max(0, deadline - time.monotonic())))
        farm.arm.set()
        while farm.epoch is None:
            if farm.error:
                raise RuntimeError(str(farm.error))
            time.sleep(.001)
        origin = farm.epoch
        stage = 'active'
        def record_gap(name, at, reason):
            # A slow frame costs one observation, not the trial. A console that keeps
            # missing its deadline still ends the run.
            gap = {'client': name, 'elapsed_s': at, 'error': reason}
            gaps.append(gap)
            logs['events'].write({'event': 'sample_gap', **gap})
            consecutive_failures[name] += 1
            if consecutive_failures[name] >= MAX_CONSECUTIVE_SAMPLE_FAILURES:
                raise RuntimeError(f'{name} console failed {consecutive_failures[name]} '
                                   f'consecutive queries: {reason}')
        events = iter([] if args.manual else scenario['events'])
        next_event = next(events, None)
        released = False
        next_sample = 0
        total = scenario['duration_s'] + scenario['recovery_s']
        with concurrent.futures.ThreadPoolExecutor(max_workers=len(consoles)) as executor:
            while (elapsed := time.monotonic() - origin) < total:
                if farm.error:
                    raise RuntimeError(str(farm.error))
                if any(p.process.poll() is not None for p in processes):
                    raise RuntimeError('a process exited during measurement')
                while next_event and next_event['at_s'] <= elapsed:
                    sent = time.monotonic() - origin
                    try:
                        reply = consoles[next_event['client']].command(next_event['command'])
                    except ConsoleTimeout as exc:
                        logs['events'].write({**next_event, 'sent_s': sent,
                            'completed_s': time.monotonic() - origin, 'error': str(exc)})
                        record_gap(next_event['client'], sent, str(exc))
                    else:
                        consecutive_failures[next_event['client']] = 0
                        logs['events'].write({**next_event, 'sent_s': sent,
                            'completed_s': time.monotonic() - origin, 'result': reply})
                    next_event = next(events, None)
                if elapsed >= scenario['duration_s'] and not released:
                    stage = 'recovery'
                    for name, console in consoles.items():
                        at = time.monotonic() - origin
                        try:
                            console.command({'type': 'release_inputs'})
                        except ConsoleTimeout as exc:
                            record_gap(name, at, str(exc))
                            continue
                        consecutive_failures[name] = 0
                        logs['events'].write({'event': 'release_for_recovery', 'client': name,
                                              'elapsed_s': time.monotonic() - origin})
                    released = True
                if elapsed >= next_sample:
                    def sample(item):
                        name, console = item
                        at = time.monotonic() - origin
                        try:
                            state = console.command({'type': 'state'})
                        except ConsoleTimeout as exc:
                            return {'client': name, 'elapsed_s': at, 'error': str(exc)}
                        return {'client': name, 'phase': 'active' if at < scenario['duration_s'] else 'recovery',
                                'elapsed_s': at, 'received_s': time.monotonic() - origin, 'state': state}
                    for row in executor.map(sample, consoles.items()):
                        if 'error' in row:
                            record_gap(row['client'], row['elapsed_s'], row['error'])
                            continue
                        consecutive_failures[row['client']] = 0
                        sample_count += 1
                        # Keep only fields needed for the bounded summary; raw states go to the capped log.
                        samples.append({**row, 'state': {key: row['state'].get(key) for key in
                            ('network', 'shots_fired', 'hits_detected', 'pending_shots', 'connection_toast', 'chassis', 'ui')}})
                        logs['samples'].write(row)
                    # Schedule against the planned instant, not the completion time, so the
                    # effective rate matches sample_hz; skipped slots are counted, not backfilled.
                    period = 1 / scenario['sample_hz']
                    next_sample += period
                    behind = time.monotonic() - origin - next_sample
                    if behind > 0:
                        missed = int(behind // period) + 1
                        schedule_skips += missed
                        next_sample += missed * period
                time.sleep(.005)
        if args.screenshot:
            for name, console in consoles.items():
                console.command({'type': 'screenshot', 'path': str(output / f'{name}.png')})
        status = 'completed'
    except (Exception, KeyboardInterrupt) as exc:
        error = f'{type(exc).__name__}: {exc}'
        logs['events'].write({'event': 'failure', 'stage': stage, 'error': error})
        print(error, file=sys.stderr, flush=True)
    finally:
        cleanup_errors = []
        for console in consoles.values():
            try:
                console.command({'type': 'release_inputs'})
                console.command({'type': 'quit'})
            except (OSError, ValueError, RuntimeError):
                pass
            try:
                console.close()
            except OSError as exc:
                cleanup_errors.append(str(exc))
        graceful_deadline = time.monotonic() + 2
        while any(p.name != 'server' and p.process.poll() is None for p in processes):
            if time.monotonic() >= graceful_deadline:
                break
            time.sleep(.05)
        for process in reversed(processes):
            try:
                process.stop()
            except (OSError, RuntimeError, subprocess.TimeoutExpired) as exc:
                cleanup_errors.append(f'{process.name}: {exc}')
        if farm:
            try:
                farm.close()
            except (OSError, RuntimeError) as exc:
                cleanup_errors.append(str(exc))
            if farm.error:
                cleanup_errors.append(f'proxy: {farm.error}')
        if cleanup_errors:
            status = 'failed'
        report = {'schema_version': 2, 'status': status, 'error': error, 'stage': stage,
                  'cleanup_errors': cleanup_errors,
                  'elapsed_total_s': time.monotonic() - started,
                  'summary_samples_retained': len(samples),
                  'summary_samples_overwritten': sample_count - len(samples),
                  'processes': [{'name': p.name, 'returncode': p.process.returncode,
                                 'cleanup': p.cleanup, 'log_dropped_bytes': p.dropped_bytes} for p in processes],
                  'diagnostic_records_dropped': {name: log.dropped for name, log in logs.items()},
                  **summarize(samples, proxies, scenario['duration_s'], gaps, schedule_skips)}
        expectations = scenario.get('expectations') or {}
        tolerance = args.baseline_tolerance if args.baseline_tolerance is not None else \
            expectations.get('baseline_tolerance', DEFAULT_BASELINE_TOLERANCE)
        report['checks'] = evaluate_checks(report, expectations) if status != 'failed' else []
        report['baseline'] = None
        if args.baseline and status != 'failed':
            try:
                reference = load_json(args.baseline)
                report['baseline'] = {'path': str(args.baseline),
                                      **compare_baseline(report, reference, tolerance)}
            except (OSError, ValueError) as exc:
                report['baseline'] = {'path': str(args.baseline), 'error': str(exc),
                                      'tolerance': tolerance, 'regressions': 0,
                                      'compared': 0, 'skipped': 0, 'comparisons': []}
                cleanup_errors.append(f'baseline: {exc}')
                report['cleanup_errors'] = cleanup_errors
                status = report['status'] = 'failed'
        for name, client in report['clients'].items():
            missed = sum(client['event_samples_missed'].values())
            report['checks'].append({'name': 'event_history_complete', 'client': name,
                                     'metric': 'event_samples_missed', 'operator': '==', 'threshold': 0,
                                     'observed': missed, 'status': 'failed' if missed else 'passed',
                                     'detail': 'event histories must cover the observation window'})
        failures = [row for row in report['checks'] if row['status'] == 'failed']
        regressions = (report['baseline'] or {}).get('regressions', 0)
        if status != 'failed':
            if failures or regressions:
                status = 'failed'
            elif expectations.get('checks') or report['baseline']:
                status = 'passed'
        report['status'] = status
        report['checks_failed'] = len(failures)
        report['baseline_regressions'] = regressions
        (output / 'summary.json').write_text(json.dumps(report, indent=2) + '\n')
        lines = [f'# Network trial: {scenario["name"]}', '', f'Status: {status}.', '',
                 f'Error: {error}' if error else 'See summary.json for distributions and metric definitions.', '',
                 '| Client | Active samples | Sample gaps | App RTT p50, ms | Checkpoint gap max, ms |',
                 '|---|---:|---:|---:|---:|']
        for name, data in report['clients'].items():
            lines.append(f'| {name} | {data["active_samples"]} | {data["sample_gaps"]} | '
                         f'{data["metrics"]["rtt_ms"]["p50"]} | {data["metrics"]["checkpoint_gap_ms"]["max"]} |')
        if report['checks']:
            lines += ['', '| Check | Client | Metric | Expected | Observed | Result |',
                      '|---|---|---|---|---:|---|']
            for row in report['checks']:
                lines.append(f'| {row["name"]} | {row["client"] or "run"} | {row["metric"]} | '
                             f'{row["operator"]} {row["threshold"]} | {row["observed"]} | {row["status"]} |')
        elif expectations.get('checks'):
            lines += ['', 'The run failed before its expectations could be evaluated.']
        else:
            lines += ['', 'No expectations were configured, so nothing was asserted.']
        if report['baseline']:
            base = report['baseline']
            lines += ['', f'Baseline {base["path"]}: {base.get("compared", 0)} statistics compared at '
                          f'{base.get("tolerance")} relative tolerance, {base.get("regressions", 0)} regressed, '
                          f'{base.get("skipped", 0)} missing on one side.']
            for row in base.get('comparisons', ()):
                if row['status'] == 'failed':
                    lines.append(f'- {row["client"]} {row["metric"]}: {row["baseline"]} -> {row["observed"]}')
        lines += ['', 'Passed means the configured expectations held; completed means the scenario ran with '
                      'none configured. Neither is a full gameplay correctness or playability verdict.',
                  'Metric availability depends on the selected build; null fields are unmeasured. Samples are not a complete event trace.']
        (output / 'summary.md').write_text('\n'.join(lines) + '\n')
        for log in logs.values():
            log.close()
    print(f'{status}: {output / "summary.md"}', flush=True)
    return 0 if status in ('completed', 'passed') else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('scenario', type=Path)
    parser.add_argument('--output', type=Path, help='New artifact directory; existing directories are refused')
    parser.add_argument('--validate', action='store_true', help='Validate the scenario and exit without launching')
    parser.add_argument('--server', type=Path, default=ROOT / 'target/debug/rm-simulator-server')
    parser.add_argument('--app', type=Path, default=ROOT / 'target/debug/rm-simulator')
    parser.add_argument('--cad-assets', type=Path, default=Path.home() / 'dev/RM/assets/rm2026-field')
    parser.add_argument('--ready-timeout', type=float, default=180)
    parser.add_argument('--console-timeout', type=float, default=CONSOLE_TIMEOUT_S,
                        help='Per-console-command deadline; a missed sample is a recorded gap')
    parser.add_argument('--manual', action='store_true', help='Visible first client; suppress scripted controls')
    parser.add_argument('--network-stats', choices=['off', 'compact', 'detailed'], help='Select the protocol 18+ client overlay')
    parser.add_argument('--screenshot', action='store_true', help='Capture each client after recovery')
    parser.add_argument('--seed', type=int, help='Override the network seed')
    parser.add_argument('--baseline', type=Path,
                        help='A prior summary.json; distribution statistics that grew beyond the '
                             'relative tolerance fail the run')
    parser.add_argument('--baseline-tolerance', type=float,
                        help=f'Relative regression tolerance for --baseline, default '
                             f'{DEFAULT_BASELINE_TOLERANCE} or the scenario value')
    parser.add_argument('--build-label', default='unspecified', help='Record build profile/toolchain details')
    args = parser.parse_args()
    try:
        data = load_json(args.scenario)
        if args.seed is not None:
            data['seed'] = args.seed
        scenario = validate_scenario(data)
        if args.validate:
            print(json.dumps(scenario, indent=2))
            return 0
        if not args.output:
            parser.error('--output is required unless using --validate')
        number(args.ready_timeout, 'ready timeout', 1, 600)
        number(args.console_timeout, 'console timeout', .05, 60)
        if args.baseline_tolerance is not None:
            number(args.baseline_tolerance, 'baseline tolerance', 0, 100)
        if args.baseline is not None:
            args.baseline = args.baseline.expanduser().resolve()
            if not args.baseline.is_file():
                raise ValueError(f'missing baseline summary: {args.baseline}')
        args.server, args.app, args.cad_assets = (path.expanduser().resolve()
                                                for path in (args.server, args.app, args.cad_assets))
        for path in (args.server, args.app, args.cad_assets / 'manifest.json',
                     args.cad_assets / 'equipment/manifest.json'):
            if not path.is_file():
                raise ValueError(f'missing required file: {path}; build binaries explicitly before running')
        def interrupted(*_):
            raise KeyboardInterrupt()
        signal.signal(signal.SIGTERM, interrupted)
        return run(args, scenario)
    except (ValueError, OSError, RuntimeError) as error:
        parser.error(str(error))


if __name__ == '__main__':
    sys.exit(main())
