# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Capture synthetic rune appearance fixtures through the real TCP client.

Start a paused headless server first. Its snapshot supplies the CAD poses;
this script varies only the rune appearance fields for visual comparison.
These captures are not tests of scoring or referee transitions.
"""

import argparse
import copy
import json
import pathlib
import socket
import subprocess
import threading

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--port", type=int, default=17770)
parser.add_argument("--cad-assets", type=pathlib.Path)
parser.add_argument(
    "--output",
    type=pathlib.Path,
    default=pathlib.Path("local-assets/scene-reference-audit"),
)
parser.add_argument(
    "--states", help="Comma-separated fixture names; default captures all states"
)
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parents[1]
out = args.output.resolve()
out.mkdir(parents=True, exist_ok=True)
with socket.create_connection(("127.0.0.1", args.port), timeout=10) as host:
    host.sendall(
        (
            json.dumps(
                {
                    "Hello": {
                        "protocol": 6,
                        "name": "scene audit",
                        "team": None,
                        "role": "Spectator",
                    }
                }
            )
            + "\n"
        ).encode()
    )
    for line in host.makefile():
        base = json.loads(line)
        if "Snapshot" in base:
            break
    else:
        raise RuntimeError("host closed without a snapshot")
(out / "input-snapshot.json").write_text(json.dumps(base, indent=2) + "\n")
variants = [
    ("inactive", "Small", 0),
    *[(f"small-{n}", "Small", n) for n in range(5)],
    ("full", "Small", 5),
    *[(f"big-{n}", "Big", n) for n in range(5)],
    ("big-first-hit", "Big", 2),
    ("big-full", "Big", 5),
]
if args.states:
    variants = [v for v in variants if v[0] in args.states.split(",")]
    if not variants:
        parser.error("no matching fixture states")
for face in ["red", "blue"]:
    for label, kind, n in variants:
        state = copy.deepcopy(base)
        field = state["Snapshot"]["field"]
        field["time_ns"] = 2_000_000_000
        field["tick"] = 2000
        for r in field["runes"]:
            r.update(
                kind=kind,
                angle_rad=0.0,
                state="Inactive"
                if label == "inactive"
                else ("Activated" if n == 5 else "Activating"),
                active_blade=None if n == 5 or label == "inactive" else n,
                active_blades=[False] * 5,
                activated=[False] * 5,
                completed_groups=n if kind == "Big" else 0,
                state_since_ns=0,
                time_ns=field["time_ns"],
            )
            if label != "inactive":
                if n == 5:
                    r["activated"] = [True] * 5
                elif kind == "Small":
                    r["activated"] = [i < n for i in range(5)]
                    r["active_blades"][n] = True
                else:
                    r["active_blades"][0] = True
                    r["active_blades"][2] = True
                    if label == "big-first-hit":
                        r["active_blades"][0] = False
                        r["activated"][0] = True
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        port = listener.getsockname()[1]

        def serve(listener=listener, state=state):
            c, _ = listener.accept()
            try:
                f = c.makefile()
                json.loads(f.readline())
                c.sendall(
                    (
                        json.dumps(
                            {
                                "Welcome": {
                                    "protocol": 6,
                                    "client_id": 0,
                                    "team": None,
                                    "role": "Spectator",
                                    "chassis": None,
                                }
                            }
                        )
                        + "\n"
                        + json.dumps(state)
                        + "\n"
                    ).encode()
                )
                for line in f:
                    msg = json.loads(line)
                    if "Ping" in msg:
                        c.sendall(
                            (
                                json.dumps(state)
                                + "\n"
                                + json.dumps({"Pong": msg["Ping"]})
                                + "\n"
                            ).encode()
                        )
            finally:
                c.close()
                listener.close()

        thread = threading.Thread(target=serve, daemon=True)
        thread.start()
        pos, yaw = (
            ("2.2,-2.2,2.60", "135") if face == "red" else ("-2.2,2.2,2.60", "315")
        )
        with open(out / f"{face}-{label}.log", "w") as log:
            subprocess.run(
                [
                    str(root / "target/debug/rm-simulator"),
                    *(
                        ["--cad-assets", str(args.cad_assets.resolve())]
                        if args.cad_assets
                        else []
                    ),
                    "--connect",
                    f"127.0.0.1:{port}",
                    "--fly",
                    "--spawn",
                    pos,
                    "--spawn-yaw-deg",
                    yaw,
                    "--rune-flash-hz",
                    "0",
                    "--screenshot",
                    str(out / f"{face}-{label}.png"),
                ],
                cwd=root,
                stdout=log,
                stderr=subprocess.STDOUT,
                check=True,
                timeout=40,
            )
        print(face, label, flush=True)
