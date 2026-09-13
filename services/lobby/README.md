<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Lobby directory

Prepared for later deployment. Public lobbies are disabled in the game and no
service is needed for LAN matches. The directory defaults to `127.0.0.1:7791`
for local development; configure `--lobby-host HOST:PORT` for a deployment.

The Python standard-library service lists player-hosted matches. It carries no
game traffic and provides no relay, NAT traversal, accounts, or passwords.
Listings expire 45 seconds after the last heartbeat. Hosts heartbeat every
10 seconds and withdraw when their session ends. Directory restarts clear all
listings; hosts can register again after a failed heartbeat.

Run tests locally:

```sh
python3 -m unittest discover -s services/lobby -v
```

When public support is enabled, deploy this directory with `docker compose up
-d --build`. The container listens on TCP 7791, runs as an unprivileged user,
and has memory, process, CPU, log and request limits. `/health` reports health.
There is no persistent data to back up.

API:

- `GET /v1/lobbies`: array of listings, including name, game address, protocol,
  transport and password-required flag.
- `POST /v1/lobbies`: register name, port, optional public IP override, protocol,
  transport and locked flag. Returns an unguessable lease token. With no IP
  override, the service uses the TCP peer's IP.
- `POST /v1/heartbeat`: renew with `{"token":"..."}`.
- `POST /v1/remove`: withdraw with the same token.

The current HTTP exchange is unencrypted. Before enabling public production
use, add HTTPS, abuse monitoring and a supported connectivity path. Tokens
are never exposed by listing responses or written to access logs. The service
bounds bodies to 2 KiB, concurrent requests to 32, listings to 512 total and
eight per source IP. Listing addresses are unverified, so future UI must not
present them as trusted or reachable. No automatic port forwarding occurs.
