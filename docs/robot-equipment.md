<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Robot equipment prototypes

The renderer constructs these meshes from boxes, chamfered housings, low-sided
cylinders and wheel rollers. It does not load the module STEP files at runtime.
The original files remain in the supplied Downloads directory. This is a visual
and driving prototype, not a competition-compliant robot design.

The `--robot` chassis preset, `--projectile-mm` caliber and view flags are in
[app options](app-options.md); [field package](field-package.md) owns the field.
The [documentation index](README.md) lists every guide.

## Reference audit

Sources were inspected on 2026-09-11. Page numbers below count PDF pages,
including the cover. Source filenames carry 2019/2020 labels, but the AM02,
SM01, FI02 and LI01 guides themselves identify v1.0, 2018.11.

| Module | Reference | Implemented approximation |
|---|---|---|
| AM02 small armor | `RoboMaster 装甲模块 AM02&AM12 2020 使用说明书.pdf`, pp.3–8; `AM02 2019.STEP` | Chamfered shell, smaller central dark panel, four exposed fasteners, rear supports, two rounded white plastic diffusers and SVG-derived white printing. Keeps the existing 141 × 135 × 19 mm physics housing and scoring face. The guide lists 140 × 125 mm; the STEP's axis-aligned envelope is approximately 19 × 130 × 141 mm. These are different envelopes, not grounds for silently changing the scoring constants. |
| AM12 large armor | Same guide, pp.4,6,8; `AM12 2019.STEP` | Evaluated, not fitted. The guide lists 235 × 127 mm. The supplied assembly is rotated, so its world-axis bounds cannot be used directly as width/height. Both prototypes retain AM02 scoring armor. A large-armor variant needs coordinated collider, scoring and appearance work. |
| LI01 light bar | `RoboMaster 裁判系统灯条模块说明书（CN&EN）.pdf`, pp.3–5; `LI01 2020 .STEP` | 285 × 50 × 84.75 mm envelope including two legs, main strip and top auxiliary windows, mounted above rear armor. Ten HP segments are a visualization choice. STEP envelope includes cable/connector protrusions. |
| FI02 field interaction | `RoboMaster 场地交互模块 FI02 2020 说明书.pdf`, pp.3–6; `FI02 2019.STEP` | Drawing dimensions 121.67 × 105.67 × 17.80 mm, four edge indicators and downward-facing detection panel. Mounted beneath the central chassis enclosure with clearance. |
| SM01 speed monitor | `RoboMaster 测速模块 SM01&SM11 使用说明书-CN&EN.pdf`, pp.3–9; `SM01 2019.STEP` | Infantry muzzle shroud, 109.81 mm long, 41.20 mm high, open projectile channel and two sensor stations. Width is an approximate drawing reading. Cable and connector protrusions are omitted. |
| SM11 speed monitor | Same guide, pp.5–6 | Hero shroud, 119.70 mm long and 73.80 mm high, with a 44 mm approximate passage for 42 mm projectiles. No SM11 STEP was supplied; this uses the guide drawing. |
| VT03 transmitter | `RoboMaster裁判系统相机图传模块VT03&VT13使用说明书.pdf`, pp.4–5,12; supplied VT03 transmitter STEP | Artist-adjusted 50 mm wide × 28 mm high × 80 mm deep case, forward lens, top cooling slots, mounting ears and short rear cable tails. Flattened and elongated at user request; these proportions are not measured VT03 dimensions. Mounted on the barrel centerline, just ahead of the cradle. The first-person eye follows the visible lens. |

The paired `-1.pdf` armor and speed-monitor downloads were not needed for this
subset. No dimensions were inferred from those duplicates.

The 2026 robot manufacturing specification V2.0.0, 20260626, was also checked:
section 3.7, Figures 3-36 through 3-38, places the speed monitor at the end of the
launcher; section 3.8, Figures 3-39 through 3-41, places FI02 beneath the chassis
and describes clearance from conductive material; section 3.9, Figures 3-42 and
3-43, identifies the current camera/transmitter arrangement. These informed
placement, not a full compliance audit. The model omits wiring, fastener
threads, connector contacts, optical calibration and electromagnetic behavior.

## Chassis assumptions

Infantry retains the existing 15 kg chassis and 153 mm omni-wheel assumptions.
Hero uses a 25 kg, 660 × 560 × 120 mm body envelope, 203 mm mecanum wheels,
65 mm wheel width and a taller, wider gimbal. These are design choices, not
rulebook dimensions. The muzzle sits 350 mm ahead of the gun pivot, leaving an exposed barrel
section behind the speed monitor. This length is an app design choice.
Both retain four small scoring modules. The visible
central enclosure and perimeter frame fit inside a conservative box collider;
individual decorative modules and rollers do not add colliders.

Hero wheel axles are parallel. Its contact-force directions are mirrored at
45 degrees and use the hub cross rolling-direction term for yaw. Wheel spin
accounts for the projection onto that direction. This extends the existing
ideal roller/suspension model; it does not model each roller contact or motor
controller. Renderer wheel centers follow suspension contacts when grounded.

The host chooses one preset for all joining pilots. The serialized chassis
configuration includes a defaulted `mecanum` flag; older snapshots without it
remain omni. Use matching client/server builds to display Hero correctly.
The referee records Hero as the robot kind but retains its configured HP.
Local Hero selection defaults the gun to 42 mm; explicit caliber overrides
still work. Remote clients select their own gun caliber. There are no new
robot-specific heat, power, ammunition or firing-rate policies.

## Lights and review fixtures

LI01 uses the referee HP fraction, rounded up to the next visible tenth.
Without a referee an alive chassis displays full HP. Defeat extinguishes LI01,
FI02, VT03 and speed-monitor indicators. Armor uses the existing per-plate hit
flash; the app's 50 ms grey flash is not a module-manual timing constant.

FI02, VT03 and speed-monitor LEDs otherwise show a static powered team color.
They do not claim a successful card read, measured velocity or radio link.
The speed monitor guide p.7 describes a waterfall effect on projectile passage;
that sequence is not implemented in live play. The comparison example stages
HP, a frozen front-plate hit flash, or defeat for visual review. The renderer
has no timer and all displayed states come from its caller.

### Armor light diffuser appearance

The supplied `RoboMaster 装甲模块 AM12 2019.STEP` was inspected with OCCT on
2026-09-11. Its rotated assembly contains paired narrow cover solids, but the
inherited solid color is the same pale blue CAD style on every component.
That styling is not a measurement of the diffuser's physical color or emission.
The off-state white plastic follows the user's reference correction.

Chassis and outpost armor now share opaque, nonmetallic, off-white diffuser
materials. Rounded shallow covers replace box-shaped or flat light strips.
The existing light-center span and tip-to-tip height remain unchanged.
Curvature, depth, roughness and reflectance are visual fits; no STEP triangles
or module geometry are embedded. Both robot prototypes still use small armor.

When powered, an intensity texture gives the team-colored emission a soft
center and edge/end falloff, leaving a reflective plastic rim. The emission
multiplier is an appearance setting, not calibrated LED radiance. Existing
camera exposure and bloom are retained. A strike uses the existing grey flash
with zero emission. Defeat removes emission while the diffuser stays white;
the printed identifier stays passive white in every state.

The powered appearance was compared against original photos in the sibling
`Vision/Yolo-Detector` repository's `data/armor_image_corpus_v2/images` corpus.
That corpus is an external source and is not retained in either checkout, so the
comparison cannot be re-run from this repository. The named references include
`armor27c_curated/gkd/images/train/4263.jpg`,
`gkd_labeled_rar/已标注数据集/已标注数据集/train/image/3676.jpg` and
`gmaster_detection_zip/XJTLU_2023_Detection_ALL/images/8469.jpg`.
These show bright, near-white bar centers surrounded by team-colored edges.
The shared emission profile now reaches that brighter center through tone
mapping. The corpus spans different exposures and includes synthetic images;
only the photographed robots guided this qualitative fit. No corpus images
are embedded or redistributed, and this is not a calibrated camera or LED model.
