#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Assert architectural dependency constraints from Cargo's resolved graph."""
import json
import subprocess

metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--format-version", "1", "--all-features", "--locked"], text=True))
packages = {p["id"]: p for p in metadata["packages"]}
nodes = {n["id"]: n["dependencies"] for n in metadata["resolve"]["nodes"]}


def dependencies(name):
    root = next(p["id"] for p in packages.values() if p["name"] == name)
    seen = set()
    pending = list(nodes[root])
    while pending:
        node = pending.pop()
        if node not in seen:
            seen.add(node)
            pending.extend(nodes[node])
    return {packages[node]["name"] for node in seen}


gameplay = dependencies("rm-simulator-gameplay")
assert not any(d.startswith(("bevy", "rapier")) for d in gameplay), gameplay
assert not any(d.startswith("rm-simulator-") for d in gameplay), gameplay
physics = dependencies("rm-simulator-physics")
assert not any(d.startswith("bevy") for d in physics), physics
assert not any(d.startswith("rm-simulator-") for d in physics), physics
world = dependencies("rm-simulator-world")
assert not any(d.startswith("bevy") for d in world), world
server = dependencies("rm-simulator-server")
assert not any(d.startswith("bevy") for d in server), server
assert "rm-simulator-render" not in server, server
render = dependencies("rm-simulator-render")
assert "rm-simulator-physics" not in render, render
assert "rm-simulator-world" not in render, render
assert "rm-simulator-server" not in render, render
assert "rm-simulator-gameplay" not in render, render
bench = dependencies("rm-simulator-bench")
assert not ({"rm-simulator-physics", "rm-simulator-world", "rm-simulator-server", "rm-simulator-gameplay", "rm-simulator-app"} & bench), bench
assert not any(d.startswith("rapier") for d in bench), bench
assert not any(p["name"] in ("rm-sim", "rm-sim-api", "rm-sim-viewer", "rm-vision-world",
                             "rm-vision-runtime", "rm-vision-runner") for p in packages.values())
print("PASS: physics has no gameplay or renderer, gameplay is independent of physics and the simulator, "
      "world and server have no renderer, renderer has no world or server, "
      "and no sibling simulator crate is imported")
