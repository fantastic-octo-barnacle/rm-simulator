# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Model invariants, real UDP delivery and runner validation without Rust builds."""
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
from network_harness_lib import Link, Profile, ProxyFarm, ServiceQueue

spec = importlib.util.spec_from_file_location('runner', SCRIPTS / 'network-harness.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class FixtureStartupTests(unittest.TestCase):
    def test_http_fixture_does_not_resolve_its_loopback_hostname(self):
        spec = importlib.util.spec_from_file_location(
            'network_peer', SCRIPTS / 'tests/fixtures/network_peer.py')
        peer = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(peer)
        with mock.patch('socket.getfqdn', side_effect=AssertionError('unexpected DNS lookup')):
            with peer.LoopbackHTTPServer(('127.0.0.1', 0), peer.http.server.BaseHTTPRequestHandler) as server:
                self.assertEqual(server.server_name, '127.0.0.1')
                self.assertGreater(server.server_port, 0)


class ModelTests(unittest.TestCase):
    def profile(self, **values):
        return Profile.parse(values)

    def test_rate_charges_headers_and_serializes_fifo(self):
        link = Link(self.profile(rate_kbps=8, overhead_bytes=28, queue_wait_ms=2000), 1, 'up')
        link.receive(b'a' * 972, 0)
        link.receive(b'b' * 972, 0)
        self.assertEqual(link.advance(.999), [])
        self.assertEqual(link.advance(1), [b'a' * 972])
        self.assertEqual(link.advance(2), [b'b' * 972])
        self.assertEqual(link.snapshot(2)['serviced_accounted_bytes'], 2000)
        self.assertEqual(link.pending_packets, 0)

    def test_path_loss_still_consumes_bandwidth(self):
        link = Link(self.profile(rate_kbps=8, overhead_bytes=0, loss_percent=100), 1, 'up')
        link.receive(b'a' * 1000, 0)
        self.assertEqual(link.advance(.5), [])
        self.assertEqual(link.stats['dropped'], 0)
        self.assertEqual(link.advance(1), [])
        self.assertEqual(link.drops['random_loss'], 1)
        self.assertEqual(link.stats['serviced_accounted_bytes'], 1000)

    def test_duplicate_is_charged_and_delivered_twice(self):
        link = Link(self.profile(rate_kbps=8, overhead_bytes=0, queue_wait_ms=2000,
                                 duplicate_percent=100), 1, 'up')
        link.receive(b'a' * 1000, 0)
        self.assertEqual(len(link.advance(1)), 1)
        self.assertEqual(len(link.advance(2)), 1)
        self.assertEqual(link.stats['duplicates_created'], 1)
        self.assertEqual(link.stats['serviced_accounted_bytes'], 2000)

    def test_queue_overflow_and_wait_have_distinct_reasons(self):
        overflow = Link(self.profile(rate_kbps=8, queue_bytes=1500, overhead_bytes=0), 1, 'up')
        overflow.receive(b'x' * 1000, 0)
        overflow.receive(b'y' * 1000, 0)
        self.assertEqual(overflow.drops['queue_overflow'], 1)
        waiting = Link(self.profile(rate_kbps=8, overhead_bytes=0, queue_wait_ms=50), 1, 'up')
        waiting.receive(b'x' * 1000, 0)
        waiting.receive(b'y', .1)
        self.assertEqual(waiting.drops['queue_wait'], 1)
        self.assertEqual(waiting.pending_bytes, 1000)
        waiting.advance(1)
        self.assertEqual(waiting.pending_bytes, 0)

    def test_propagation_follows_service(self):
        link = Link(self.profile(rate_kbps=8, overhead_bytes=0, delay_ms=200), 1, 'up')
        link.receive(b'a' * 1000, 0)
        self.assertEqual(link.advance(1), [])
        self.assertEqual(link.snapshot(1)['queue_bytes'], 0)
        self.assertEqual(link.pending_packets, 1)
        self.assertEqual(len(link.advance(1.2)), 1)

    def test_blackout_discards_in_flight_and_new_packets_without_replay(self):
        link = Link(self.profile(delay_ms=2000, blackouts=[[.5, .5]]), 1, 'up')
        link.receive(b'in-flight', 0)
        link.receive(b'during', .6)
        self.assertEqual(link.advance(2), [])
        self.assertEqual(link.drops['blackout'], 2)
        link.receive(b'fresh', 2)
        self.assertEqual(link.advance(4), [b'fresh'])

    def test_blackout_end_is_exclusive(self):
        link = Link(self.profile(blackouts=[[1, 1]]), 1, 'up')
        link.receive(b'start', 1)
        link.receive(b'end', 2)
        self.assertEqual(link.advance(2), [b'end'])
        self.assertEqual(link.drops['blackout'], 1)

    def test_rate_changes_affect_new_admissions_not_existing_reservations(self):
        link = Link(self.profile(rate_kbps=8, overhead_bytes=0, queue_wait_ms=2000,
                                 rate_changes=[{'at_s':.5, 'rate_kbps':16}]), 1, 'up')
        link.receive(b'a' * 1000, 0)
        link.receive(b'b' * 1000, .5)
        self.assertEqual(len(link.advance(1)), 1)
        self.assertEqual(len(link.advance(1.5)), 1)

    def test_switch_to_unlimited_does_not_overtake_reserved_packets(self):
        link = Link(self.profile(rate_kbps=8, overhead_bytes=0, queue_wait_ms=2000,
                                 rate_changes=[{'at_s':.5, 'rate_kbps':None}]), 1, 'up')
        link.receive(b'a' * 1000, 0)
        link.receive(b'b', .5)
        self.assertEqual(link.advance(.9), [])
        self.assertEqual(link.advance(1), [b'a' * 1000, b'b'])

    def test_shared_egress_is_aggregate_across_peers(self):
        shared = (ServiceQueue(), self.profile(rate_kbps=8, overhead_bytes=0, queue_wait_ms=2000))
        a, b = [Link(self.profile(overhead_bytes=0), 1, name) for name in ('a', 'b')]
        a.receive(b'a' * 1000, 0, shared)
        b.receive(b'b' * 1000, 0, shared)
        self.assertEqual(a.advance(.9), [])
        self.assertEqual(len(a.advance(1)), 1)
        self.assertEqual(b.advance(1), [])
        self.assertEqual(len(b.advance(2)), 1)

    def test_shared_queue_overflow_is_attributed(self):
        shared = (ServiceQueue(), self.profile(rate_kbps=8, queue_bytes=1000))
        link = Link(self.profile(overhead_bytes=0), 1, 'up')
        link.receive(b'a' * 1000, 0, shared)
        link.receive(b'b', 0, shared)
        self.assertEqual(link.drops['shared_queue_overflow'], 1)

    def trace(self, seed, label, **extra):
        link = Link(self.profile(loss_percent=10, jitter_ms=50, reorder_percent=30,
                                 reorder_delay_ms=80, burst_good_ms=200, burst_bad_ms=100,
                                 **extra), seed, label)
        trace = []
        for index in range(500):
            now = index / 100
            trace.extend((now, payload) for payload in link.advance(now))
            link.receive(str(index).encode(), now)
        trace.extend((6, payload) for payload in link.advance(6))
        return trace, dict(link.drops)

    def test_seeded_trace_is_repeatable_and_directionally_independent(self):
        a = self.trace(19, 'up')
        self.assertEqual(a, self.trace(19, 'up'))
        self.assertNotEqual(a, self.trace(19, 'down'))
        self.assertNotEqual(a, self.trace(20, 'up'))
        self.assertGreater(a[1]['burst_loss'], 0)
        delivered = [int(payload) for _, payload in a[0]]
        self.assertNotEqual(delivered, sorted(delivered))

    def test_pending_packet_limit_bounds_zero_byte_datagrams(self):
        link = Link(self.profile(delay_ms=1000), 1, 'up')
        for _ in range(9000):
            link.receive(b'', 0)
        self.assertEqual(link.pending_packets, 8192)
        self.assertEqual(link.drops['pending_limit'], 808)
        self.assertEqual(len(link.advance(1)), 8192)
        self.assertEqual(link.pending_packets, 0)

    def test_counters_balance_including_duplicates_and_socket_drops(self):
        link = Link(self.profile(duplicate_percent=100, loss_percent=30), 1, 'up')
        for index in range(100):
            link.receive(b'bytes', index / 100)
        for index, payload in enumerate(link.advance(2)):
            link.delivered(payload, success=index % 2 == 0)
        stats = link.snapshot(2)
        self.assertEqual(stats['received'] + stats['duplicates_created'],
                         stats['forwarded'] + stats['dropped'] + stats['pending_packets'])
        self.assertEqual(stats['forwarded_bytes'] + stats['dropped_bytes'], 1000)

    def test_profile_rejects_invalid_values(self):
        cases = [{'rate_kbps':0}, {'delay_ms':float('nan')}, {'loss_percent':101},
                 {'queue_bytes':-1}, {'queue_bytes':1.5}, {'delay_ms':True}, {'typo':1},
                 {'burst_bad_ms':10}, {'burst_good_ms':.001, 'burst_bad_ms':1},
                 {'blackouts':[[1,-1]]}, {'rate_changes':[{'at_s':1,'rate_kbps':0}]}]
        for case in cases:
            with self.subTest(case=case), self.assertRaises(ValueError):
                Profile.parse(case)


class UdpTests(unittest.TestCase):
    def test_real_udp_rate_cap_paces_packets(self):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
            server.bind(('127.0.0.1', 0))
            server.settimeout(2)
            proxy = ProxyFarm([{'name':'pilot','target_port':server.getsockname()[1],
                                'upstream':{'rate_kbps':64,'overhead_bytes':0,'queue_wait_ms':2000}}]).start()
            try:
                for _ in range(10):
                    client.sendto(b'x'*500,('127.0.0.1',proxy.ports[0]))
                arrivals = []
                for _ in range(10):
                    self.assertEqual(len(server.recvfrom(1000)[0]),500)
                    arrivals.append(time.monotonic())
                self.assertGreaterEqual(arrivals[-1]-arrivals[0],.50)
                self.assertLess(arrivals[-1]-arrivals[0],1.5)
            finally:
                proxy.close()

    def test_legacy_cli_blackout_clock_starts_at_first_packet(self):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
            server.bind(('127.0.0.1',0))
            server.settimeout(.2)
            process = subprocess.Popen([sys.executable,str(SCRIPTS/'network-impairment.py'),
                '--transport','udp','--listen-port','0','--target-port',str(server.getsockname()[1]),
                '--delay-ms','0','--jitter-ms','0','--blackout-duration-ms','100',
                '--blackout-after-ms','0','--blackout-direction','upstream'],
                stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
            try:
                ready = json.loads(process.stdout.readline())
                time.sleep(.15)
                client.sendto(b'first',('127.0.0.1',ready['listen_port']))
                with self.assertRaises(socket.timeout):
                    server.recvfrom(100)
                client.sendto(b'after',('127.0.0.1',ready['listen_port']))
                self.assertEqual(server.recvfrom(100)[0],b'after')
                process.terminate()
                stdout,stderr = process.communicate(timeout=3)
                self.assertEqual(process.returncode,0,stderr)
                self.assertTrue(any(json.loads(line).get('final') for line in stdout.splitlines()))
            finally:
                if process.poll() is None:
                    process.kill()
                    process.communicate(timeout=3)

    def test_real_udp_delay_loss_direction_and_cleanup(self):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
            server.bind(('127.0.0.1', 0))
            server.settimeout(1)
            client.settimeout(.15)
            proxy = ProxyFarm([{'name':'pilot', 'target_port':server.getsockname()[1],
                                'upstream':{'delay_ms':30}, 'downstream':{'loss_percent':100}}]).start()
            port = proxy.ports[0]
            try:
                start = time.monotonic()
                client.sendto(b'hello', ('127.0.0.1', port))
                payload, peer = server.recvfrom(100)
                self.assertEqual(payload, b'hello')
                self.assertGreaterEqual(time.monotonic() - start, .025)
                server.sendto(b'reply', peer)
                with self.assertRaises(socket.timeout):
                    client.recvfrom(100)
            finally:
                proxy.close()
            self.assertIsNone(proxy.error)
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as replacement:
                replacement.bind(('127.0.0.1', port))

    def test_foreign_client_cannot_steal_pinned_endpoint(self):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as first, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as second:
            server.bind(('127.0.0.1', 0))
            server.settimeout(.2)
            proxy = ProxyFarm([{'name':'pilot','target_port':server.getsockname()[1]}]).start()
            try:
                first.sendto(b'first', ('127.0.0.1', proxy.ports[0]))
                self.assertEqual(server.recvfrom(100)[0], b'first')
                second.sendto(b'foreign', ('127.0.0.1', proxy.ports[0]))
                with self.assertRaises(socket.timeout):
                    server.recvfrom(100)
                self.assertEqual(proxy.foreign_packets, 1)
            finally:
                proxy.close()

    def test_manual_arming_excludes_startup_and_begins_impairment(self):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as client:
            server.bind(('127.0.0.1', 0))
            server.settimeout(.2)
            proxy = ProxyFarm([{'name':'pilot','target_port':server.getsockname()[1],
                                'upstream':{'loss_percent':100}}], armed=False).start()
            try:
                client.sendto(b'setup', ('127.0.0.1', proxy.ports[0]))
                self.assertEqual(server.recvfrom(100)[0], b'setup')
                self.assertIsNone(proxy.epoch)
                proxy.arm.set()
                deadline = time.monotonic() + 1
                while proxy.epoch is None and time.monotonic() < deadline:
                    time.sleep(.001)
                self.assertIsNotNone(proxy.epoch)
                client.sendto(b'active', ('127.0.0.1', proxy.ports[0]))
                with self.assertRaises(socket.timeout):
                    server.recvfrom(100)
            finally:
                proxy.close()


class RunnerTests(unittest.TestCase):
    def scenario(self):
        return {'schema_version':1,'name':'test','clients':[{'name':'pilot'}]}

    def test_all_shipped_scenarios_validate(self):
        for path in (SCRIPTS / 'network-scenarios').glob('*.json'):
            runner.validate_scenario(json.loads(path.read_text()))

    def test_bad_scenario_rejects_unknown_client_and_fields(self):
        for change in ({'clients':[]}, {'typo':1}, {'sample_hz':0}, {'seed':False},
                       {'events':[{'at_s':0,'client':'other','command':{'type':'capture'}}]},
                       {'shared_downstream':{'loss_percent':1}}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                runner.validate_scenario({**self.scenario(), **change})

    def test_percentiles_and_absent_values(self):
        self.assertIsNone(runner.distribution([])['p50'])
        self.assertIsNone(runner.distribution([1,2])['p95'])
        self.assertEqual(runner.distribution(range(1,101))['p95'], 95)
        self.assertEqual(runner.distribution(range(1,101))['p99'], 99)

    def test_summary_rates_use_actual_elapsed_and_ignore_recovery(self):
        def row(at, count):
            return {'elapsed_s':at,'peers':{'pilot':{'upstream':{
                'received_bytes':count,'forwarded_bytes':count,'serviced_accounted_bytes':count,'dropped':0}}}}
        report = runner.summarize([], [row(.2,100),row(2.2,1100),row(9,100000)], 3)
        self.assertAlmostEqual(report['proxy_rates']['pilot']['upstream']['received_bytes_per_s'],500)
        self.assertIn('same_tick_correction_error', report['unavailable'])

    def test_owned_process_cleanup_does_not_stop_unrelated_process(self):
        unrelated = subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)'])
        try:
            with tempfile.TemporaryDirectory() as tmp:
                owned = runner.OwnedProcess('owned',[sys.executable,'-c','import time; time.sleep(30)'],Path(tmp))
                owned.stop()
                self.assertIsNotNone(owned.process.poll())
                self.assertIsNone(unrelated.poll())
        finally:
            unrelated.terminate()
            unrelated.wait(timeout=3)

    def test_trace_cap_drops_records_instead_of_blocking(self):
        with tempfile.TemporaryDirectory() as tmp:
            old = runner.MAX_ARTIFACT
            runner.MAX_ARTIFACT = 32
            log = runner.JsonLog(Path(tmp)/'test.jsonl')
            try:
                log.write({'a':'b'})
                log.write({'large':'x'*100})
                self.assertEqual(log.dropped,1)
                self.assertLessEqual(log.bytes,32)
            finally:
                log.close()
                runner.MAX_ARTIFACT = old


def sample(client, elapsed, network=None, phase='active', **state):
    """One reduced sample row in the shape `summarize` retains."""
    return {'client':client, 'phase':phase, 'elapsed_s':elapsed, 'received_s':elapsed+.001,
            'state':{'network':network if network is not None else {}, **state}}


class ExpectationTests(unittest.TestCase):
    def scenario(self, **expectations):
        return {'schema_version':1, 'name':'test', 'clients':[{'name':'pilot'},{'name':'peer'}],
                'expectations':{'checks':[{'name':'gate','metric':'rtt_ms.p50','op':'<=','value':50}],
                                **expectations}}

    def report(self, gaps=()):
        rows = []
        for index in range(30):
            rows.append(sample('pilot', index * .25, {
                'event_samples':{'rtt_ms':{'total':index, 'values':[(index,10+index)]}},
                'rtt_ms':10+index, 'checkpoint_gap_ms':40., 'unresolved_shot_outcomes':0,
                'remote_underruns':index, 'shot_confirmation_ms':12.5,
                'shot_execution_offset_ms':-2., 'collision_context_age_ms':4.,
                'collision_context_coherent':index % 2 == 0,
                'prediction_results_discarded':index, 'prediction_held_moving_frames':2 * index,
                'stats':{'loss_percent':1.5, 'native':{'transport_rtt_ms':8.}}},
                shots_fired=index, hits_detected=0, pending_shots=1))
            rows.append(sample('peer', index * .25, {'event_samples':{'rtt_ms':{'total':index, 'values':[(index,200.)]}}, 'rtt_ms':200., 'unresolved_shot_outcomes':index}))
        return runner.summarize(rows, [], 10, gaps)

    def test_schema_accepts_a_documented_block(self):
        scenario = runner.validate_scenario(self.scenario(baseline_tolerance=.4))
        self.assertEqual(scenario['expectations']['baseline_tolerance'], .4)
        self.assertEqual(scenario['expectations']['checks'][0]['on_missing'], 'fail')

    def test_schema_rejects_invalid_checks(self):
        cases = [{'checks':[]}, {'checks':[{'name':'a','metric':'rtt_ms','op':'~','value':1}]},
                 {'checks':[{'name':'A','metric':'rtt_ms','op':'<','value':1}]},
                 {'checks':[{'name':'a','metric':'rtt_ms','op':'<','value':'1'}]},
                 {'checks':[{'name':'a','metric':'rtt ms','op':'<','value':1}]},
                 {'checks':[{'name':'a','metric':'rtt_ms','op':'<','value':1,'client':'ghost'}]},
                 {'checks':[{'name':'a','metric':'rtt_ms','op':'<','value':1,
                             'per_client':True,'client':'pilot'}]},
                 {'checks':[{'name':'a','metric':'rtt_ms','op':'<','value':1,'on_missing':'ignore'}]},
                 {'checks':[{'name':'a','metric':'rtt_ms','op':'<','value':1,'typo':1}]},
                 {'checks':[{'name':'a','metric':'rtt_ms','op':'<','value':1},
                            {'name':'a','metric':'rtt_ms','op':'>','value':0}]},
                 {'baseline_tolerance':-1}, {'typo':1}]
        for case in cases:
            with self.subTest(case=case), self.assertRaises(ValueError):
                runner.validate_scenario(self.scenario(**case))

    def test_shipped_scenarios_declare_the_correctness_gates(self):
        for path in (SCRIPTS / 'network-scenarios').glob('*.json'):
            scenario = runner.validate_scenario(json.loads(path.read_text()))
            checks = {check['name']: check for check in scenario['expectations']['checks']}
            unresolved = checks['no_unresolved_shot_outcomes']
            self.assertEqual((unresolved['metric'], unresolved['op'], unresolved['value'],
                              unresolved['per_client']),
                             ('unresolved_shot_outcomes_delta', '==', 0, True))
            self.assertEqual(checks['checkpoint_gap_bounded']['metric'], 'checkpoint_gap_ms.max')

    def evaluate(self, report, checks):
        return runner.evaluate_checks(report, {'checks':[
            runner.validate_expectations({'checks':[check]}, {'pilot','peer'})['checks'][0]
            for check in checks]})

    def test_per_client_check_evaluates_every_client(self):
        rows = self.evaluate(self.report(), [
            {'name':'gate','metric':'unresolved_shot_outcomes_delta','op':'==','value':0,
             'per_client':True}])
        self.assertEqual({row['client']: row['status'] for row in rows},
                         {'pilot':'passed', 'peer':'failed'})
        self.assertEqual([row['observed'] for row in rows if row['client'] == 'peer'], [29])

    def test_named_client_check_evaluates_only_that_client(self):
        rows = self.evaluate(self.report(), [
            {'name':'gate','metric':'rtt_ms.p50','op':'<=','value':50,'client':'pilot'}])
        self.assertEqual([(row['client'], row['status']) for row in rows], [('pilot','passed')])

    def test_run_scope_check_reads_the_summary_root(self):
        rows = self.evaluate(self.report([{'client':'pilot','elapsed_s':1,'error':'slow'}]),
                             [{'name':'gate','metric':'sample_gap_total','op':'<=','value':0}])
        self.assertEqual([(row['client'], row['status'], row['observed']) for row in rows],
                         [(None, 'failed', 1)])

    def test_missing_metric_fails_unless_skipped(self):
        report = self.report()
        failing = self.evaluate(report, [{'name':'gate','metric':'absent_metric','op':'<=',
                                          'value':1,'per_client':True}])
        self.assertEqual({row['status'] for row in failing}, {'failed'})
        self.assertIn('absent', failing[0]['detail'])
        skipped = self.evaluate(report, [{'name':'gate','metric':'correction_position_m.p95',
                                          'op':'<=','value':1,'per_client':True,
                                          'on_missing':'skip'}])
        self.assertEqual({row['status'] for row in skipped}, {'skipped'})

    def test_a_per_client_check_over_no_clients_does_not_pass(self):
        rows = self.evaluate(runner.summarize([], [], 10),
                             [{'name':'gate','metric':'active_samples','op':'>=','value':1,
                               'per_client':True}])
        self.assertEqual([(row['client'], row['status']) for row in rows], [(None, 'failed')])

    def test_prefixed_and_shorthand_metric_paths_agree(self):
        report = self.report()
        for path in ('rtt_ms.p50', 'metrics.rtt_ms.p50'):
            rows = self.evaluate(report, [{'name':'gate','metric':path,'op':'<=','value':50,
                                           'client':'pilot'}])
            self.assertEqual(rows[0]['status'], 'passed', path)


class SummaryFieldTests(unittest.TestCase):
    def report(self, **kwargs):
        rows = [sample('pilot', index * .25, {
            'event_samples':{key:{'total':index, 'values':[(index,value)]}
                             for key,value in [('shot_confirmation_ms',index),('shot_execution_offset_ms',-1.)]},
            'shot_confirmation_ms':index, 'shot_execution_offset_ms':-1.,
            'collision_context_age_ms':3., 'collision_context_coherent':index < 15,
            'prediction_results_discarded':index, 'prediction_held_moving_frames':3 * index,
            'stats':{'loss_percent':2., 'native':{'transport_rtt_ms':5.}}},
            shots_fired=index, hits_detected=2 * index, pending_shots=index % 3)
            for index in range(20)]
        return runner.summarize(rows, [], 10, **kwargs)

    def test_console_fields_the_old_summary_ignored_are_collected(self):
        metrics = self.report()['clients']['pilot']['metrics']
        for key in ('shot_confirmation_ms', 'shot_execution_offset_ms', 'collision_context_age_ms',
                    'transport_loss_percent', 'pending_shots'):
            self.assertEqual(metrics[key]['count'], 19 if key.startswith('shot_') else 20, key)
        self.assertEqual(metrics['shot_confirmation_ms']['max'], 19)

    def test_counter_deltas_include_prediction_and_context_coherence(self):
        client = self.report()['clients']['pilot']
        self.assertEqual(client['prediction_results_discarded_delta'], 19)
        self.assertEqual(client['prediction_held_moving_frames_delta'], 57)
        self.assertEqual(client['observed_global_hits_detected_delta'], 38)
        self.assertAlmostEqual(client['collision_context_coherent_fraction'], .75)

    def test_unavailable_reflects_what_was_collected(self):
        unavailable = self.report()['unavailable']
        self.assertNotIn('per_shot_execution_and_confirmation_delay', unavailable)
        self.assertNotIn('transport_loss', unavailable)
        self.assertNotIn('transport_rtt', unavailable)
        self.assertIn('same_tick_correction_error', unavailable)
        self.assertIn('remote_presentation', unavailable)
        self.assertIn('hit_disagreement', unavailable)

    def test_sample_gaps_are_counted_per_client_and_for_the_run(self):
        report = self.report(gaps=[{'client':'pilot','elapsed_s':1,'error':'slow'},
                                   {'client':'absent','elapsed_s':2,'error':'slow'}],
                             schedule_skips=3)
        self.assertEqual(report['sample_gap_total'], 2)
        self.assertEqual(report['sample_schedule_skips'], 3)
        self.assertEqual(report['clients']['pilot']['sample_gaps'], 1)
        self.assertEqual(report['clients']['absent']['sample_gaps'], 1)
        self.assertEqual(report['clients']['absent']['active_samples'], 0)


class EventMeasurementTests(unittest.TestCase):
    def rows(self, histories):
        return [sample('pilot', i, {'event_samples':{'shot_confirmation_ms':history},
                                   'shot_confirmation_ms':999.})
                for i, history in enumerate(histories)]

    def test_repeated_polls_do_not_duplicate_events_but_equal_values_do(self):
        first = {'total':2, 'values':[(1,900.),(2,900.)]}
        next_ = {'total':4, 'values':[(1,900.),(2,900.),(3,10.),(4,10.)]}
        result, missed, resets = runner.event_distribution(self.rows([first,next_,next_,next_]), 'shot_confirmation_ms')
        self.assertEqual((result['count'],result['p50'],missed), (2,10.,0))
        self.assertIsNone(result['p95'])

    def test_overwrite_is_reported_and_missing_poll_preserves_cursor(self):
        rows = self.rows([{'total':1,'values':[(1,5.)]}, None,
                          {'total':5,'values':[(4,6.),(5,7.)]}])
        result, missed, resets = runner.event_distribution(rows, 'shot_confirmation_ms')
        self.assertEqual((result['count'],missed), (2,2))

    def test_legacy_last_values_are_not_per_event_measurements(self):
        report = runner.summarize([sample('pilot',0,{'shot_confirmation_ms':30.}),
                                   sample('pilot',1,{'shot_confirmation_ms':30.})],[],2)
        self.assertEqual(report['clients']['pilot']['metrics']['shot_confirmation_ms']['count'],0)
        self.assertIn('per_shot_execution_and_confirmation_delay',report['unavailable'])

    def test_counter_reset_preserves_summary_and_records_failure_metadata(self):
        rows = self.rows([{'total':2,'values':[]}, {'total':1,'values':[]},
                          {'total':2,'values':[(2,7.)]}])
        report = runner.summarize(rows, [], 3)
        client = report['clients']['pilot']
        self.assertEqual(client['event_history_resets']['shot_confirmation_ms'], 1)
        self.assertEqual(client['metrics']['shot_confirmation_ms']['count'], 1)
        json.dumps(report)

    def test_old_last_value_summary_cannot_be_compared_as_events(self):
        old = {'clients':{'pilot':{'metrics':{'rtt_ms':{'p50':1.,'max':2.}}}}}
        new = {**old, 'event_metric_schema':1}
        result = runner.compare_baseline(new,old,.25)
        self.assertEqual(result['compared'],0)
        self.assertEqual(result['regressions'],0)
        self.assertEqual(result['comparisons'][0]['detail'],'incompatible event measurement schema')

    def test_freshness_median_does_not_gate_arrival_latency(self):
        old = {'clients':{'pilot':{'metrics':{'checkpoint_gap_ms':{'p50':.5,'max':30.}}}}}
        new = {'clients':{'pilot':{'metrics':{'checkpoint_gap_ms':{'p50':17.,'max':60.}}}}}
        result = runner.compare_baseline(new,old,.25)
        self.assertEqual(result['regressions'],1)  # The maximum freshness age still gates.


class BaselineTests(unittest.TestCase):
    def report(self, values):
        return {'clients':{'pilot':{'metrics':{'rtt_ms':values}}}}

    def test_growth_beyond_tolerance_is_a_regression(self):
        result = runner.compare_baseline(self.report({'p50':130, 'p95':None, 'p99':None, 'max':None}),
                                         self.report({'p50':100, 'p95':None, 'p99':None, 'max':None}),
                                         .25)
        self.assertEqual(result['regressions'], 1)
        failed = [row for row in result['comparisons'] if row['status'] == 'failed']
        self.assertEqual((failed[0]['metric'], failed[0]['baseline'], failed[0]['observed']),
                         ('rtt_ms.p50', 100, 130))

    def test_growth_inside_tolerance_and_improvement_pass(self):
        for observed in (124, 100, 10):
            result = runner.compare_baseline(
                self.report({'p50':observed, 'p95':None, 'p99':None, 'max':None}),
                self.report({'p50':100, 'p95':None, 'p99':None, 'max':None}), .25)
            self.assertEqual(result['regressions'], 0, observed)
            self.assertEqual(result['compared'], 1)

    def test_missing_metric_on_either_side_is_skipped(self):
        result = runner.compare_baseline(self.report({'p50':500, 'p95':None, 'p99':None, 'max':None}),
                                         {'clients':{}}, .25)
        self.assertEqual((result['regressions'], result['compared'], result['skipped']), (0, 0, 4))
        self.assertEqual({row['detail'] for row in result['comparisons']}, {'missing in baseline'})
        result = runner.compare_baseline(self.report({'p50':None, 'p95':None, 'p99':None, 'max':None}),
                                         self.report({'p50':1, 'p95':None, 'p99':None, 'max':None}), .25)
        self.assertEqual((result['regressions'], result['compared']), (0, 0))

    def test_a_previously_zero_statistic_may_not_grow(self):
        result = runner.compare_baseline(self.report({'p50':.5, 'p95':None, 'p99':None, 'max':None}),
                                         self.report({'p50':0, 'p95':None, 'p99':None, 'max':None}),
                                         .25)
        self.assertEqual(result['regressions'], 1)


class RunnerProcessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.fixture = self.root / 'peer'
        source = (SCRIPTS / 'tests/fixtures/network_peer.py').read_text()
        self.fixture.write_text(source.replace('#!/usr/bin/env python3', '#!' + sys.executable))
        self.fixture.chmod(0o755)
        assets = self.root / 'assets'
        (assets / 'equipment').mkdir(parents=True)
        (assets / 'manifest.json').write_text('{}')
        (assets / 'equipment/manifest.json').write_text('{}')
        self.scenario = self.root / 'scenario.json'
        self.scenario.write_text(json.dumps({'schema_version':1,'name':'fixture','warmup_s':0,
            'duration_s':.6,'recovery_s':.1,'sample_hz':10,
            'shared_downstream':{'rate_kbps':128},
            'clients':[{'name':'healthy'},{'name':'impaired','downstream':{'loss_percent':50}}],
            'events':[{'at_s':0,'client':'healthy','command':{'type':'capture','captured':True}}]}))
        self.output = self.root / 'report'
        self.command = [sys.executable,str(SCRIPTS/'network-harness.py'),str(self.scenario),
                        '--output',str(self.output),'--server',str(self.fixture),'--app',str(self.fixture),
                        '--cad-assets',str(assets),'--ready-timeout','5','--build-label','test fixture']

    def tearDown(self):
        self.temp.cleanup()

    def assert_cleaned(self):
        manifest = json.loads((self.output/'manifest.json').read_text())
        for process in manifest.get('processes', []):
            with self.assertRaises(ProcessLookupError):
                os.kill(process['pid'], 0)
        for port in manifest.get('ports', {}).get('proxy', []):
            with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as sock:
                sock.bind(('127.0.0.1',port))

    def rewrite(self, **changes):
        data = json.loads(self.scenario.read_text())
        data.update(changes)
        self.scenario.write_text(json.dumps(data))

    def run_harness(self, *extra, env=None, timeout=30):
        environment = {**os.environ, **(env or {})}
        result = subprocess.run([*self.command, *extra], capture_output=True, text=True,
                                timeout=timeout, env=environment)
        return result, json.loads((self.output/'summary.json').read_text())

    def test_event_reset_writes_failed_summary_and_cleans_processes(self):
        source = self.fixture.read_text()
        source = source.replace("'total':replies[0]", "'total':1000-replies[0]")
        source = source.replace("'values':[(i,42) for i in range(max(1,replies[0]-255),replies[0]+1)]",
                                "'values':[]")
        self.fixture.write_text(source)
        result, report = self.run_harness()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(report['status'], 'failed')
        checks = [check for check in report['checks'] if check['name'] == 'event_history_complete']
        self.assertTrue(checks)
        self.assertTrue(all(check['status'] == 'failed' and check['resets'] > 0 for check in checks))
        self.assert_cleaned()

    def test_loopback_readiness_ignores_proxy_settings(self):
        # Reserve a non-listening port: using this proxy must fail immediately.
        with socket.socket() as proxy:
            proxy.bind(('127.0.0.1', 0))
            address = f'http://127.0.0.1:{proxy.getsockname()[1]}'
            result, report = self.run_harness(env={
                'http_proxy': address, 'HTTP_PROXY': address,
                'all_proxy': address, 'ALL_PROXY': address,
                'no_proxy': '', 'NO_PROXY': '',
            })
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(report['status'], 'completed')
        self.assert_cleaned()

    def test_expectations_that_hold_report_passed(self):
        # The fixture console answers rtt_ms 42 and always resolves shot outcomes.
        self.rewrite(expectations={'checks':[
            {'name':'rtt_bounded','metric':'rtt_ms.p50','op':'<=','value':100,'per_client':True},
            {'name':'sampled','metric':'active_samples','op':'>=','value':1,'per_client':True},
            {'name':'no_gaps','metric':'sample_gap_total','op':'==','value':0}]})
        result, report = self.run_harness()
        self.assertEqual(result.returncode, 0, result.stdout+result.stderr)
        self.assertEqual(report['status'], 'passed')
        self.assertEqual(report['checks_failed'], 0)
        self.assertEqual({row['status'] for row in report['checks']}, {'passed'})
        self.assertEqual(len([row for row in report['checks'] if row['name'] == 'rtt_bounded']), 2)
        self.assert_cleaned()

    def test_failed_expectation_fails_the_run(self):
        self.rewrite(expectations={'checks':[
            {'name':'rtt_bounded','metric':'rtt_ms.p50','op':'<=','value':1,'per_client':True},
            {'name':'named_peer','metric':'active_samples','op':'>=','value':1,
             'client':'impaired'}]})
        result, report = self.run_harness()
        self.assertEqual(result.returncode, 1, result.stdout+result.stderr)
        self.assertEqual(report['status'], 'failed')
        self.assertEqual(report['checks_failed'], 2)
        failed = [row for row in report['checks'] if row['status'] == 'failed']
        self.assertEqual({row['observed'] for row in failed}, {42})
        self.assertEqual([row['status'] for row in report['checks']
                          if row['name'] == 'named_peer'], ['passed'])
        self.assertIn('failed', (self.output/'summary.md').read_text())
        self.assert_cleaned()

    def test_baseline_regression_fails_and_improvement_passes(self):
        baseline = self.root / 'baseline.json'
        stats = {'p50':1, 'p95':None, 'p99':None, 'max':1}
        baseline.write_text(json.dumps({'event_metric_schema':1,
                                        'clients':{name:{'metrics':{'rtt_ms':stats}}
                                                   for name in ('healthy','impaired')}}))
        result, report = self.run_harness('--baseline', str(baseline))
        self.assertEqual(result.returncode, 1, result.stdout+result.stderr)
        self.assertEqual(report['status'], 'failed')
        self.assertGreaterEqual(report['baseline_regressions'], 1)
        self.assertEqual(report['baseline']['tolerance'], .25)
        generous = self.root / 'generous.json'
        generous.write_text(baseline.read_text())
        self.output.rename(self.root / 'previous')
        result, report = self.run_harness('--baseline', str(generous),
                                          '--baseline-tolerance', '100')
        self.assertEqual(result.returncode, 0, result.stdout+result.stderr)
        self.assertEqual(report['status'], 'passed')
        self.assertEqual(report['baseline_regressions'], 0)
        self.assertGreater(report['baseline']['skipped'], 0)

    def test_slow_console_records_a_gap_instead_of_aborting(self):
        self.rewrite(duration_s=2, clients=[{'name':'healthy'}], events=[])
        result, report = self.run_harness('--console-timeout', '.2',
                                          env={'RM_FIXTURE_STALL_AFTER':'3',
                                               'RM_FIXTURE_STALL_S':'.6',
                                               'RM_FIXTURE_STALL_COUNT':'1'})
        self.assertEqual(result.returncode, 0, result.stdout+result.stderr)
        self.assertEqual(report['status'], 'completed')
        self.assertGreaterEqual(report['sample_gap_total'], 1)
        self.assertGreaterEqual(report['clients']['healthy']['sample_gaps'], 1)
        self.assertGreater(report['clients']['healthy']['active_samples'], 3)
        self.assert_cleaned()

    def test_repeatedly_failing_console_fails_the_run(self):
        self.rewrite(duration_s=8, clients=[{'name':'healthy'}], events=[])
        result, report = self.run_harness('--console-timeout', '.2',
                                          env={'RM_FIXTURE_STALL_AFTER':'2',
                                               'RM_FIXTURE_STALL_S':'30',
                                               'RM_FIXTURE_STALL_COUNT':'0'})
        self.assertEqual(result.returncode, 1, result.stdout+result.stderr)
        self.assertEqual(report['status'], 'failed')
        self.assertIn('consecutive queries', report['error'])
        self.assertEqual(report['clients']['healthy']['sample_gaps'],
                         runner.MAX_CONSECUTIVE_SAMPLE_FAILURES)
        self.assert_cleaned()

    def test_sampling_holds_its_planned_rate(self):
        self.rewrite(duration_s=2, recovery_s=0, sample_hz=10, clients=[{'name':'healthy'}],
                     events=[])
        result, report = self.run_harness()
        self.assertEqual(result.returncode, 0, result.stdout+result.stderr)
        # 10 Hz over two active seconds; scheduling against completion time lost roughly
        # a third of these before the fix.
        self.assertGreaterEqual(report['clients']['healthy']['active_samples'], 17)
        self.assertEqual(report['sample_schedule_skips'], 0)

    def test_real_runner_two_clients_reports_and_cleans_up(self):
        result = subprocess.run(self.command,capture_output=True,text=True,timeout=20)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        report = json.loads((self.output/'summary.json').read_text())
        self.assertEqual(report['status'],'completed')
        self.assertEqual(set(report['clients']),{'healthy','impaired'})
        for client in report['clients'].values():
            self.assertGreater(client['active_samples'],0)
        self.assertEqual(report['cleanup_errors'],[])
        self.assert_cleaned()

    def test_startup_failure_produces_report(self):
        self.fixture.write_text('#!' + sys.executable + '\nraise SystemExit(7)\n')
        result = subprocess.run(self.command,capture_output=True,text=True,timeout=20)
        self.assertEqual(result.returncode,1,result.stdout+result.stderr)
        report = json.loads((self.output/'summary.json').read_text())
        self.assertEqual(report['status'],'failed')
        self.assertIn('startup',report['error'])
        self.assertEqual(report['processes'][0]['returncode'],7)

    def test_readiness_timeout_stops_owned_server(self):
        self.fixture.write_text('#!' + sys.executable + '\nimport time\ntime.sleep(30)\n')
        command = [*self.command,'--ready-timeout','1']
        result = subprocess.run(command,capture_output=True,text=True,timeout=10)
        self.assertEqual(result.returncode,1,result.stdout+result.stderr)
        report = json.loads((self.output/'summary.json').read_text())
        self.assertIn('readiness timed out',report['error'])
        self.assertIsNotNone(report['processes'][0]['returncode'])

    def test_interrupt_records_failure_and_cleans_up(self):
        data = json.loads(self.scenario.read_text())
        data['duration_s'] = 60
        self.scenario.write_text(json.dumps(data))
        process = subprocess.Popen(self.command,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        try:
            deadline = time.monotonic()+10
            samples = self.output/'samples.jsonl'
            while (not samples.exists() or samples.stat().st_size == 0) and time.monotonic()<deadline:
                if process.poll() is not None:
                    self.fail(str(process.communicate()))
                time.sleep(.02)
            self.assertTrue(samples.exists() and samples.stat().st_size > 0)
            process.terminate()
            stdout, stderr = process.communicate(timeout=10)
            self.assertEqual(process.returncode,1,stdout+stderr)
            report = json.loads((self.output/'summary.json').read_text())
            self.assertIn('KeyboardInterrupt',report['error'])
            self.assert_cleaned()
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate(timeout=3)


if __name__ == '__main__':
    unittest.main()
