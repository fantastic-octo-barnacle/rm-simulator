<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Optional Steam build

Ordinary `cargo build` and `just run` do not link or initialize Steam. The app's
`steam` feature adds client initialization and a callback pump on the main thread.
It does not add Steam matchmaking, authentication, achievements, cloud storage or
Steam Input; the existing multiplayer protocol and controls remain in charge.

The app initializes Steam before starting Bevy's worker threads. The upstream
`Client::init_app` sets process environment variables, so it must stay in this
early startup path. The app retains one client until shutdown and calls
`Client::run_callbacks` each frame, following the
[steamworks client API](https://docs.rs/steamworks/0.13.1/steamworks/struct.Client.html).
If initialization fails, it prints the reason and continues without Steam.

App ID selection uses `RM_SIMULATOR_STEAM_APP_ID`, then Steam's `SteamAppId`.
Debug builds use Spacewar's ID 480 when neither is set. Release builds have no
fallback ID. For distribution, supply your application's ID; the packaging script
requires `--development` to accept 480 and never includes `steam_appid.txt`.

## Windows limitation

The current Steam-enabled app does not link on Windows MSVC. The static
GameNetworkingSockets library and `steam_api64.lib` define overlapping
`SteamAPI_*` networking symbols, producing LNK2005 and LNK1169 errors.
The standalone Windows release does not enable Steam and builds successfully.
Native CI runs its workspace tests without optional features on Windows and
checks all-feature code with `cargo check`, which does not link an executable.
Linux and macOS retain all-feature tests. A Windows Steam package requires
resolving the native library collision first.

## Build and stage the runtime

This script stages the executable and Steam runtime for development and local
testing. It does not produce a portable distribution. Run on the target operating
system:

```sh
python3 scripts/package-steam.py --app-id YOUR_APP_ID --output dist/steam
```

For a local development package:

```sh
python3 scripts/package-steam.py --profile dev --app-id 480 --development --output dist/steam-dev
./dist/steam-dev/run.sh --cad-assets /path/to/rm2026-field
```

On Windows use `run.cmd`. On Linux and macOS use `run.sh`. The scripts set the App
ID and locate the executable beside the matching Steam runtime. Steam client
availability is separate from the runtime library: a Steam-enabled executable
needs its packaged runtime library even if Steam is not running.

The package contains the app, Steam runtime, launcher and a manifest of their
SHA-256 checksums and native dependency inventory. It also carries `NOTICE.md`,
`LICENSE-MIT`, `LICENSE-APACHE` and `LICENSES/MPL-2.0.txt`, because a binary that
embeds the MPL-2.0 CSS crates has to ship that notice and source offer. The
manifest keys are package-relative paths, so the nested `LICENSES/` entry is
distinct from the files beside the launcher. Field assets remain external.
The existing GNS build also needs its protobuf/OpenSSL runtime libraries. On the
current macOS development build these resolve to Homebrew paths; this script does
not relocate or bundle them. Install those prerequisites for local testing. A
portable distribution still needs native dependency packaging and platform signing. The output directory must not
already exist. Packaging builds the native host target with `Cargo.lock`; it does
not cross-compile, sign an app bundle, upload a depot or publish to Steam.

## SDK compatibility

The locked `steamworks` 0.13.1 dependency uses `steamworks-sys` 0.13.0 and its
bundled SDK. Packaging uses that exact runtime by default. It verifies the API
metadata against `scripts/steam-sdk-api.sha256`, and any explicit `--sdk` runtime
must also match the bundled library's checksum. An inherited
`STEAM_SDK_LOCATION` does not override packaging; pass `--sdk` explicitly.

The separately installed Valve SDK 1.65 stays in ignored
`vendor/steamworks-sdk/` for tools and reference. Its SteamUtils011 interface does
not match these bindings' SteamUtils010 interface. Do not substitute its runtime
for the crate's bundled runtime. The package script and the app's build script
reject its API metadata. Updating the Rust bindings requires updating the SDK
contract and checking packaging again.

```sh
python3 scripts/package-steam.py --check-sdk-only
# An explicit mismatched SDK fails before building or packaging:
python3 scripts/package-steam.py --check-sdk-only --sdk vendor/steamworks-sdk
```

For direct Cargo use, `cargo build -p rm-simulator-app --features steam` uses the
crate's bundled SDK unless `STEAM_SDK_LOCATION` is set. The build script validates
that override. Prefer the package script when launching the executable, because
it places the required runtime library beside it.
