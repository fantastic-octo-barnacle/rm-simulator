# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
set shell := ["sh", "-cu"]
set quiet
set minimum-version := "1.55.0"

[default]
help:
    just --list

fmt:
    cargo fmt --all

check:
    cargo check --workspace --all-targets --all-features --locked

lint:
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

test:
    cargo test --workspace --all-features --locked

deny:
    cargo deny check advisories bans licenses sources

hooks:
    prek run --all-files --show-diff-on-failure

module-deps:
    python3 scripts/check-module-dependencies.py

mpl:
    python3 scripts/check-mpl-compliance.py

verify:
    prek run --all-files --show-diff-on-failure
    cargo fmt --all -- --check
    cargo check --workspace --all-targets --all-features --locked
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    cargo test --workspace --all-features --locked
    python3 scripts/check-module-dependencies.py
    python3 scripts/check-mpl-compliance.py
    python3 -m unittest discover -s scripts/tests -p 'test_network*.py'
    python3 -m unittest discover -s scripts/tests -p 'test_render_benchmark.py'
    python3 -m unittest discover -s scripts/tests -p 'test_mpl_compliance.py'
    python3 -m unittest discover -s scripts/tests -p 'test_release*.py'
    python3 -m unittest discover -s scripts/tests -p 'test_ci.py'
    python3 -m doctest scripts/check-mpl-compliance.py
    cargo deny check advisories bans licenses sources

# Standalone network harness tests, including real UDP and process cleanup.
network-test:
    python3 -m unittest discover -s scripts/tests -p 'test_network*.py' -v

# Run an isolated scenario using explicitly built server/app binaries.
network-trial SCENARIO *ARGS:
    python3 scripts/network-harness.py {{quote(SCENARIO)}} {{ARGS}}

# Gameplay tests without physics, CAD, a server or a renderer.
gameplay-test:
    cargo test -p rm-simulator-gameplay --locked

# A complete headless gameplay scenario.
gameplay-demo:
    cargo run -p rm-simulator-gameplay --example match --locked

# Renderer-independent world tests only.
world-test:
    cargo test -p rm-simulator-world --locked

# Interactive first-person scene.
run *ARGS:
    cargo run -p rm-simulator-app --locked -- {{ARGS}}

# Headless simulation server (GNS UDP plus the HTTP referee panel).
server *ARGS:
    cargo run -p rm-simulator-server --locked -- {{ARGS}}

# Build once, then run rendering sweeps without compiling during capture.
bench-build:
    cargo build -p rm-simulator-bench --locked

bench-render CONFIG *ARGS:
    python3 scripts/benchmark-render.py {{quote(CONFIG)}} {{ARGS}}
