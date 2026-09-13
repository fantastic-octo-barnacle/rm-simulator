<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Security policy

## Supported versions

Security fixes target the latest commit on `main`. Older revisions and releases
are not maintained separately.

## Report a vulnerability

Email [hxyulin@proton.me](mailto:hxyulin@proton.me) privately. Do not open a
public issue or pull request with exploit details or exposed credentials.

Include the affected commit or version, operating system, simulator mode and
transport, steps to reproduce, and the potential impact. A small reproduction
or sanitized log is helpful. Do not send live credentials, personal data,
external CAD packages or the Steamworks SDK.

The maintainer will review the report and coordinate any fix and disclosure
with you. There is no guaranteed response time or paid bounty program.

## Deployment scope

This project is in development. Public lobby discovery is disabled. The optional
lobby directory uses HTTP, and TCP game handshakes carry lobby passwords without
encryption. Use a match-specific password, not one reused for another service.
Keep referee and console endpoints restricted to trusted access.

See [multiplayer networking](docs/multiplayer-networking.md) and the
[lobby service documentation](services/lobby/README.md) for the current transport
and deployment limitations.
