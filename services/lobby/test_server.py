# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
import json
import threading
import unittest
import urllib.error
import urllib.request
from unittest.mock import patch
import server


class DirectoryTests(unittest.TestCase):
    def setUp(self):
        server.LOBBIES.clear()
        self.http = server.Server(('127.0.0.1', 0), server.Handler)
        self.worker = threading.Thread(target=self.http.serve_forever, daemon=True)
        self.worker.start()
        self.url = f'http://127.0.0.1:{self.http.server_port}'

    def tearDown(self):
        self.http.shutdown()
        self.http.server_close()
        self.worker.join()

    def request(self, path, data=None):
        request = urllib.request.Request(self.url + path, data=None if data is None else json.dumps(data).encode())
        try:
            with urllib.request.urlopen(request, timeout=3) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    def entry(self):
        # Public IP validation fixture only; no traffic is sent to this address.
        return dict(name='Test lobby', port=7700, protocol=25, transport='gns', locked=True, ip='8.8.8.8')

    def test_lease_authentication_expiry_and_no_password_disclosure(self):
        with patch.object(server.time, 'monotonic', return_value=100):
            status, receipt = self.request('/v1/lobbies', self.entry())
            self.assertEqual(status, 201)
            entries = self.request('/v1/lobbies')[1]
            self.assertEqual(len(entries), 1)
            self.assertTrue(entries[0]['locked'])
            self.assertNotIn('token', entries[0])
            self.assertNotIn('password', entries[0])
            self.assertEqual(self.request('/v1/remove', {'token': 'wrong'})[0], 404)
        with patch.object(server.time, 'monotonic', return_value=130):
            self.assertEqual(self.request('/v1/heartbeat', receipt)[0], 200)
        with patch.object(server.time, 'monotonic', return_value=150):
            self.assertEqual(len(self.request('/v1/lobbies')[1]), 1)
        with patch.object(server.time, 'monotonic', return_value=176):
            self.assertEqual(self.request('/v1/lobbies')[1], [])
            self.assertEqual(self.request('/v1/heartbeat', receipt)[0], 404)

    def test_validation_capacity_and_withdrawal(self):
        for key, value in [('name', '\n'), ('port', 0), ('protocol', True), ('transport', 'bad'), ('ip', '192.168.1.1'), ('locked', 'yes')]:
            data = self.entry()
            data[key] = value
            self.assertEqual(self.request('/v1/lobbies', data)[0], 400)
        receipts = [self.request('/v1/lobbies', self.entry())[1] for _ in range(8)]
        self.assertEqual(self.request('/v1/lobbies', self.entry())[0], 429)
        self.assertEqual(self.request('/v1/remove', receipts[0])[0], 200)
        self.assertEqual(len(self.request('/v1/lobbies')[1]), 7)
        self.assertEqual(self.request('/v1/lobbies', [1])[0], 400)


if __name__ == '__main__':
    unittest.main()
