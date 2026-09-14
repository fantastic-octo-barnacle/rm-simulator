# T3 completion-order results

One fixed ablation on the reused T2 development seed. No production adoption or independent validation claim.

| Robots | Link | Policy | p95 checkpoint ms | p95 chassis ms | p95 Pong ms | p95 projectile error m | Missing projectiles | Downstream bytes |
|---|---|---|---:|---:|---:|---:|---:|---:|
| 2 | clean | rr-128 | 120 | 30 | 0 | 0.897 | 1.04% | 4848268 |
| 2 | clean | drr-128-211 | 120 | 30 | 0 | 0.897 | 1.04% | 4848092 |
| 2 | clean | completion-128-211 | 120 | 30 | 0 | 0.897 | 1.04% | 4848104 |
| 2 | rtt | rr-128 | 172 | 84 | 60 | 2.059 | 2.90% | 4884907 |
| 2 | rtt | drr-128-211 | 172 | 84 | 60 | 2.059 | 2.90% | 4886039 |
| 2 | rtt | completion-128-211 | 172 | 84 | 60 | 2.059 | 2.90% | 4884743 |
| 2 | limited | rr-128 | 798 | 184 | 1306 | 8.313 | 11.75% | 2457591 |
| 2 | limited | drr-128-211 | 874 | 162 | 1134 | 6.242 | 8.81% | 2457611 |
| 2 | limited | completion-128-211 | 664 | 162 | 1102 | 6.285 | 8.83% | 2458446 |
| 12 | clean | rr-128 | 120 | 30 | 8 | 0.896 | 1.04% | 9639530 |
| 12 | clean | drr-128-211 | 120 | 30 | 8 | 0.896 | 1.04% | 9637646 |
| 12 | clean | completion-128-211 | 120 | 30 | 8 | 0.896 | 1.04% | 9636202 |
| 12 | rtt | rr-128 | 172 | 84 | 144 | 2.055 | 2.90% | 9677595 |
| 12 | rtt | drr-128-211 | 172 | 84 | 144 | 2.055 | 2.90% | 9676995 |
| 12 | rtt | completion-128-211 | 172 | 84 | 144 | 2.055 | 2.90% | 9674303 |
| 12 | limited | rr-128 | 1732 | 474 | 3608 | 11.078 | 16.10% | 2457911 |
| 12 | limited | drr-128-211 | 2222 | 382 | 2898 | 7.254 | 10.30% | 2457975 |
| 12 | limited | completion-128-211 | 1820 | 382 | 2828 | 7.293 | 10.30% | 2457227 |
