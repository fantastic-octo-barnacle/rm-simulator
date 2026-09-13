<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Rendering effects on the RTX 3070 Ti Laptop

Shadows were the largest adjustable cost at 1080p. At 4K, the main render and post-processing stages occupied more of the frame, so reducing shadow quality saved a smaller fraction. Occlusion culling made this open-field view slower at both resolutions.

Measured on 2026-09-12 with an i9-12900H, RTX 3070 Ti Laptop 8 GB, 48 GB RAM, Arch Linux and NVIDIA 615.71.09 through native Vulkan. The laptop was on AC power. Its fans were already at maximum according to the user; NVIDIA reported software thermal slowdown near 87°C and GPU clocks varied. These are sustained-load observations, not fixed-clock laboratory results or Mac predictions.

The effect sweep contains 56 fresh-process runs: 14 cases at two resolutions, each repeated twice in a seeded shuffled order. Every run used the same fixed camera, stadium and 355,661-triangle `field-coarse` package, with five seconds of warmup and ten seconds of sampling. Rendering was offscreen to preserve the requested physical resolution. All 132,668 GPU frame captures completed without dropped, invalid or pending readbacks. No physics, robots, networking or gameplay UI ran.

The table uses the mean of the two per-run GPU medians. Percentages compare that value with the original Ultra baseline at the same resolution; negative means less GPU time. Two repetitions and variable clocks do not establish a confidence interval. Small differences, particularly the inconsistent shadow-distance result, should not drive preset changes.

| Change from original Ultra | 1080p GPU ms | Change | 4K GPU ms | Change |
|---|---:|---:|---:|---:|
| Original Ultra | 3.056 | +0.0% | 7.012 | +0.0% |
| Shadows off | 1.026 | -66.4% | 4.638 | -33.9% |
| One shadow cascade | 1.603 | -47.5% | 6.028 | -14.0% |
| Two shadow cascades | 2.518 | -17.6% | 6.625 | -5.5% |
| 2048 px shadows | 2.356 | -22.9% | 6.657 | -5.1% |
| 1024 px shadows | 2.056 | -32.7% | 6.614 | -5.7% |
| 40 m shadow distance | 3.232 | +5.8% | 6.166 | -12.1% |
| MSAA off | 2.854 | -6.6% | 5.889 | -16.0% |
| 2× MSAA | 3.071 | +0.5% | 6.171 | -12.0% |
| Bloom off | 2.761 | -9.6% | 6.442 | -8.1% |
| Depth prepass on | 3.160 | +3.4% | 6.403 | -8.7% |
| Occlusion culling on | 4.240 | +38.8% | 9.616 | +37.1% |
| Sparse geometry experiment | 2.183 | -28.6% | 5.585 | -20.3% |
| Stadium removed | 2.461 | -19.5% | 5.567 | -20.6% |

Sparse geometry changes coverage, overdraw and shadows along with triangle count; its improvement is not a pure measure of geometry processing. Removing the stadium also changes the rendered scene. Neither experiment changes the shipped map. Disabling bloom saved roughly 8–10%, but it remains enabled above Low to retain the appearance of lights and markings. The depth prepass result depends on resolution, and occlusion added about 37–39%, so both remain opt-in.

The confirmation sweep adds 16 runs and 46,021 complete GPU captures. It compares full Medium, High and Ultra settings at 1080p, plus Ultra at 4K, with ten seconds each of warmup and sampling. Only cascade count differs within each pair.

| Preset | Original cascades | Selected cascades | Original GPU ms | Selected GPU ms | GPU time reduction |
|---|---:|---:|---:|---:|---:|
| Medium 1080p | 2 | 1 | 1.485 | 1.305 | 12.2% |
| High 1080p | 2 | 1 | 1.803 | 1.470 | 18.5% |
| Ultra 1080p | 4 | 2 | 3.319 | 2.503 | 24.6% |
| Ultra 4K | 4 | 2 | 7.381 | 7.011 | 5.0% |

The 4K Ultra difference is within the run-to-run spread, so it is not a firm 4K improvement claim. The larger 1080p reductions support the cascade change. Frame-loop timing also includes CPU work and submission pacing; reducing GPU time does not imply the same percentage increase in game FPS.

Medium and High now use one cascade; Ultra uses two. Shadow-map sizes, shadow distances, MSAA and bloom stay at their previous values. Low remains shadow-free. A single cascade spreads its shadow-map texels across the whole shadow distance, making nearby shadow edges coarser; screenshots of the fixed view were inspected for that tradeoff. Users can restore two or four cascades through the existing override. No map or collision geometry changed.

Mac measurement is deferred at the user’s request to test NVIDIA first. These results do not establish the Mac’s 120 FPS target, identify its bottleneck, or establish production readiness for the full simulation.

Reproduce the experiments with [render-effects.json](../benchmarks/render-effects.json) and [render-preset-cascades.json](../benchmarks/render-preset-cascades.json), following [the benchmark instructions](render-benchmark.md). Both configurations explicitly pin the measured rendering options so later preset changes do not silently change their baselines. The [result summary](../benchmarks/results/nvidia-3070ti-20260912.json) retains individual run statistics, stage means, asset hashes and the executable hash. Full reports, screenshots and telemetry are retained under `local-assets/benchmark-results/laptop-effects-20260912` and `laptop-presets-20260912` on the development machine.

The desktop resized the earlier requested 4K window to 1920×1126. The earlier 4K offscreen probe also dropped most timestamp samples with the old 16-slot pool. Those runs are excluded here. The benchmark now checks the actual camera target size, uses 256 readback slots, and fails runs that drop samples. It does not silently substitute CPU time for GPU time.

Validation included formatting, strict Clippy checks, targeted app/benchmark tests,
the sweep-driver tests and crate-boundary checks. A resized-window rejection was
verified to exit cleanly after pipeline warmup, and the rebuilt NVIDIA executable
was checked with the new High default and complete timestamp readbacks.
