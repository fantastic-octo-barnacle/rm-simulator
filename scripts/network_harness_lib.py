# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Bounded UDP impairment model and loopback proxy, using only the standard library."""
import collections
import dataclasses
import hashlib
import heapq
import json
import math
import random
import selectors
import socket
import threading
import time


MAX_PENDING_BYTES = 8 << 20
MAX_PENDING_PACKETS = 8192


def number(value, label, minimum=0, maximum=None):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise ValueError(f'{label} must be a finite number')
    if value < minimum or (maximum is not None and value > maximum):
        raise ValueError(f'{label} is outside its allowed range')
    return value


@dataclasses.dataclass(frozen=True)
class Profile:
    delay_ms: float = 0
    jitter_ms: float = 0
    loss_percent: float = 0
    rate_kbps: float = None
    queue_bytes: int = 256 * 1024
    queue_wait_ms: float = 250
    overhead_bytes: int = 28
    duplicate_percent: float = 0
    reorder_percent: float = 0
    reorder_delay_ms: float = 0
    burst_good_ms: float = 0
    burst_bad_ms: float = 0
    burst_loss_percent: float = 100
    blackouts: tuple = ()
    rate_changes: tuple = ()

    @classmethod
    def parse(cls, data):
        if not isinstance(data, dict):
            raise ValueError('profile must be an object')
        unknown = set(data) - {field.name for field in dataclasses.fields(cls)}
        if unknown:
            raise ValueError(f'unknown profile fields: {sorted(unknown)}')
        result = cls(**data)
        for field in dataclasses.fields(cls):
            value = getattr(result, field.name)
            if field.name in ('blackouts', 'rate_changes') or (field.name == 'rate_kbps' and value is None):
                continue
            number(value, field.name, maximum=100 if field.name.endswith('percent') else None)
        if result.rate_kbps is not None and result.rate_kbps <= 0:
            raise ValueError('rate_kbps must be positive or null for unlimited')
        for key in ('queue_bytes', 'overhead_bytes'):
            if not isinstance(getattr(result, key), int):
                raise ValueError(f'{key} must be an integer')
        if result.queue_bytes > MAX_PENDING_BYTES or result.queue_bytes <= 0:
            raise ValueError('queue_bytes must be between 1 and 8388608')
        if result.overhead_bytes > 256:
            raise ValueError('overhead_bytes must be at most 256')
        if bool(result.burst_good_ms) != bool(result.burst_bad_ms):
            raise ValueError('burst_good_ms and burst_bad_ms must both be positive or both zero')
        if result.burst_good_ms and min(result.burst_good_ms, result.burst_bad_ms) < 1:
            raise ValueError('burst mean durations must be at least 1 ms')
        if not isinstance(result.blackouts, (list, tuple)) or len(result.blackouts) > 1000:
            raise ValueError('blackouts must be a list of at most 1000 intervals')
        if not isinstance(result.rate_changes, (list, tuple)) or len(result.rate_changes) > 1000:
            raise ValueError('rate_changes must be a list of at most 1000 changes')
        for interval in result.blackouts:
            if not isinstance(interval, list) or len(interval) != 2:
                raise ValueError('blackouts must contain [start_s, duration_s] pairs')
            number(interval[0], 'blackout start')
            number(interval[1], 'blackout duration', minimum=0.001)
        previous = -1
        for change in result.rate_changes:
            if not isinstance(change, dict) or set(change) != {'at_s', 'rate_kbps'}:
                raise ValueError('rate_changes require at_s and rate_kbps')
            number(change['at_s'], 'rate change time')
            if change['at_s'] <= previous:
                raise ValueError('rate changes must have strictly increasing times')
            previous = change['at_s']
            if change['rate_kbps'] is not None:
                number(change['rate_kbps'], 'changed rate', minimum=0.001)
        return result

    def overlaps_blackout(self, start, end):
        return any(start < at + duration and end >= at for at, duration in self.blackouts)

    def rate_at(self, now):
        rate = self.rate_kbps
        for change in self.rate_changes:
            if change['at_s'] > now:
                break
            rate = change['rate_kbps']
        return rate


class ServiceQueue:
    """FIFO reservations include headers; a fake monotonic clock drives all work."""
    def __init__(self):
        self.reservations = collections.deque()
        self.tail = 0.0
        self.bytes = 0
        self.peak_bytes = 0
        self.peak_wait_s = 0.0

    def expire(self, now):
        while self.reservations and self.reservations[0][0] <= now:
            _, size = self.reservations.popleft()
            self.bytes -= size

    def reserve(self, now, size, profile):
        self.expire(now)
        rate = profile.rate_at(now)
        wait = max(0, self.tail - now)
        if rate is None and wait == 0:
            return now, None
        if self.bytes + size > profile.queue_bytes:
            return None, 'queue_overflow'
        if wait > profile.queue_wait_ms / 1000:
            return None, 'queue_wait'
        end = max(now, self.tail) + (size * 8 / (rate * 1000) if rate else 0)
        self.tail = end
        self.bytes += size
        self.peak_bytes = max(self.peak_bytes, self.bytes)
        self.peak_wait_s = max(self.peak_wait_s, wait)
        self.reservations.append((end, size))
        return end, None


class Link:
    """Path loss follows service, duplicates pay service, jitter may reorder delivery."""
    def __init__(self, profile, seed, label):
        self.profile = profile
        self.random = {kind: random.Random(int.from_bytes(hashlib.sha256(
            f'{seed}/{label}/{kind}'.encode()).digest()[:8], 'big'))
            for kind in ('loss', 'jitter', 'duplicate', 'reorder', 'burst')}
        self.queue = ServiceQueue()
        self.events = []
        self.serial = 0
        self.pending_bytes = 0
        self.pending_packets = 0
        self.stats = collections.Counter()
        self.drops = collections.Counter()
        self.drop_bytes = collections.Counter()
        self.bad = False
        self.burst_transition = self.burst_duration() if profile.burst_good_ms else math.inf

    def burst_duration(self):
        mean = self.profile.burst_bad_ms if self.bad else self.profile.burst_good_ms
        return max(0.000001, self.random['burst'].expovariate(1000 / mean))

    def drop(self, reason, size):
        self.stats['dropped'] += 1
        self.stats['dropped_bytes'] += size
        self.drops[reason] += 1
        self.drop_bytes[reason] += size

    def schedule(self, at, stage, payload, started):
        self.serial += 1
        heapq.heappush(self.events, (at, self.serial, stage, payload, started))

    def receive(self, payload, now, shared=None):
        self.stats['received'] += 1
        self.stats['received_bytes'] += len(payload)
        copies = 1 + (self.random['duplicate'].random() * 100 < self.profile.duplicate_percent)
        self.stats['duplicates_created'] += copies - 1
        for _ in range(copies):
            size = len(payload)
            if self.profile.overlaps_blackout(now, now):
                self.drop('blackout', size)
                continue
            if self.pending_bytes + size > MAX_PENDING_BYTES or self.pending_packets >= MAX_PENDING_PACKETS:
                self.drop('pending_limit', size)
                continue
            self.pending_bytes += size
            self.pending_packets += 1
            if shared:
                end, reason = shared[0].reserve(now, size + self.profile.overhead_bytes, shared[1])
                if reason:
                    self.finish_drop('shared_' + reason, size)
                    continue
                self.schedule(end, 'shared', payload, now)
            else:
                self.admit(payload, now, now)

    def admit(self, payload, now, started):
        end, reason = self.queue.reserve(now, len(payload) + self.profile.overhead_bytes, self.profile)
        if reason:
            self.finish_drop(reason, len(payload))
        else:
            self.schedule(end, 'service', payload, started)

    def finish_drop(self, reason, size):
        self.pending_bytes -= size
        self.pending_packets -= 1
        self.drop(reason, size)

    def advance(self, now):
        ready = []
        while self.events and self.events[0][0] <= now:
            at, _, stage, payload, started = heapq.heappop(self.events)
            if stage == 'shared':
                self.admit(payload, at, started)
                continue
            if stage == 'service':
                self.stats['serviced'] += 1
                self.stats['serviced_accounted_bytes'] += len(payload) + self.profile.overhead_bytes
                while self.burst_transition <= at:
                    self.bad = not self.bad
                    self.burst_transition += self.burst_duration()
                loss = self.profile.burst_loss_percent if self.bad else self.profile.loss_percent
                if self.random['loss'].random() * 100 < loss:
                    self.finish_drop('burst_loss' if self.bad else 'random_loss', len(payload))
                    continue
                delay = self.profile.delay_ms + self.random['jitter'].uniform(0, self.profile.jitter_ms)
                if self.random['reorder'].random() * 100 < self.profile.reorder_percent:
                    delay += self.profile.reorder_delay_ms
                self.schedule(at + delay / 1000, 'deliver', payload, started)
            elif self.profile.overlaps_blackout(started, at):
                self.finish_drop('blackout', len(payload))
            else:
                self.pending_packets -= 1
                self.pending_bytes -= len(payload)
                ready.append(payload)
        self.queue.expire(now)
        return ready

    def delivered(self, payload, success=True):
        if success:
            self.stats['forwarded'] += 1
            self.stats['forwarded_bytes'] += len(payload)
        else:
            self.drop('socket_send', len(payload))

    def snapshot(self, now):
        self.queue.expire(now)
        return {
            **{key: self.stats[key] for key in ('received', 'received_bytes', 'forwarded',
                'forwarded_bytes', 'dropped', 'dropped_bytes', 'duplicates_created',
                'serviced', 'serviced_accounted_bytes')},
            'drop_reasons': dict(self.drops), 'drop_reason_bytes': dict(self.drop_bytes),
            'pending_bytes': self.pending_bytes, 'pending_packets': self.pending_packets,
            'queue_bytes': self.queue.bytes,
            'queue_delay_ms': max(0, self.queue.tail - now) * 1000,
            'peak_queue_bytes': self.queue.peak_bytes,
            'peak_queue_wait_ms': self.queue.peak_wait_s * 1000,
        }


class ProxyFarm:
    """One worker owns isolated loopback UDP endpoints; optional shared server egress."""
    def __init__(self, peers, seed=2026, shared_downstream=None, report=None, armed=True,
                 start_on_packet=False):
        self.selector = selectors.DefaultSelector()
        self.peers = []
        self.seed = seed
        self.report = report or (lambda sample: None)
        self.shared = (ServiceQueue(), Profile.parse(shared_downstream)) if shared_downstream else None
        self.stop = threading.Event()
        self.arm = threading.Event()
        if armed:
            self.arm.set()
        self.epoch = None
        self.error = None
        self.start_on_packet = start_on_packet
        self.thread = None
        self.foreign_packets = 0
        try:
            for spec in peers:
                front = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                back = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                peer = {'name': spec['name'], 'front': front, 'back': back, 'address': None,
                        'links': {direction: Link(Profile.parse(spec.get(direction, {})), seed,
                                                 spec['name'] + '/' + direction)
                                  for direction in ('upstream', 'downstream')}}
                self.peers.append(peer)
                front.bind(('127.0.0.1', spec.get('listen_port', 0)))
                back.connect(('127.0.0.1', spec['target_port']))
                for sock, direction in ((front, 'upstream'), (back, 'downstream')):
                    sock.setblocking(False)
                    self.selector.register(sock, selectors.EVENT_READ, (peer, direction))
        except BaseException:
            self.close()
            raise

    @property
    def ports(self):
        return [peer['front'].getsockname()[1] for peer in self.peers]

    def start(self):
        self.thread = threading.Thread(target=self.run, name='rm-network-proxy', daemon=True)
        self.thread.start()
        return self

    def send(self, peer, direction, payload):
        try:
            if direction == 'upstream':
                peer['back'].send(payload)
            elif peer['address'] is not None:
                peer['front'].sendto(payload, peer['address'])
            else:
                return False
            return True
        except OSError:
            return False

    def snapshot(self, now):
        result = {'schema_version': 1, 'elapsed_s': now, 'foreign_packets': self.foreign_packets,
                  'peers': {peer['name']: {direction: link.snapshot(now)
                            for direction, link in peer['links'].items()} for peer in self.peers}}
        if self.shared:
            self.shared[0].expire(now)
            result['shared_downstream'] = {
                'queue_bytes': self.shared[0].bytes,
                'queue_delay_ms': max(0, self.shared[0].tail - now) * 1000,
                'peak_queue_bytes': self.shared[0].peak_bytes,
            }
        return result

    def run(self):
        report_at = 0
        try:
            while not self.stop.is_set():
                if self.epoch is None and self.arm.is_set() and not self.start_on_packet:
                    self.epoch = time.monotonic()
                now = time.monotonic() - self.epoch if self.epoch is not None else 0
                # Advance queued events before new admissions, even during a receive flood.
                for peer in self.peers:
                    for direction, link in peer['links'].items():
                        for payload in link.advance(now):
                            link.delivered(payload, self.send(peer, direction, payload))
                due = min((link.events[0][0] for peer in self.peers
                           for link in peer['links'].values() if link.events), default=now + 0.01)
                for event, _ in self.selector.select(min(0.01, max(0, due - now))):
                    peer, direction = event.data
                    try:
                        payload, source = event.fileobj.recvfrom(65535)
                    except (BlockingIOError, ConnectionRefusedError):
                        continue
                    if direction == 'upstream':
                        if peer['address'] is None:
                            peer['address'] = source
                        if peer['address'] != source:
                            self.foreign_packets += 1
                            continue
                    if self.epoch is None and self.arm.is_set() and self.start_on_packet:
                        self.epoch = time.monotonic()
                    if self.epoch is None:
                        self.send(peer, direction, payload)
                    else:
                        at = time.monotonic() - self.epoch
                        peer['links'][direction].receive(payload, at,
                            self.shared if direction == 'downstream' else None)
                if self.epoch is not None and now >= report_at:
                    self.report(self.snapshot(now))
                    report_at = now + 1
            if self.epoch is not None:
                self.report({**self.snapshot(time.monotonic() - self.epoch), 'final': True})
        except BaseException as error:
            self.error = error
            self.stop.set()

    def close(self):
        self.stop.set()
        if self.thread:
            self.thread.join(timeout=3)
            if self.thread.is_alive():
                raise RuntimeError('proxy worker did not stop')
        self.selector.close()
        for peer in self.peers:
            peer['front'].close()
            peer['back'].close()


def load_json(path):
    def reject_constant(value):
        raise ValueError(f'non-finite JSON value: {value}')
    with open(path, encoding='utf8') as stream:
        return json.load(stream, parse_constant=reject_constant)
