# Live2D on iced — working spike

A minimal, **running** proof that a Live2D Cubism model can be rendered inside an
[iced](https://github.com/iced-rs/iced) desktop application with pure Rust — no
Cubism Core binary, no C++ toolchain, no engine.

- `iced` master `e5c350f` (`0.15.0-dev`) + default `wgpu` renderer
- `mocari` `0.4.1` (MIT) for `.moc3` parsing, motion/expression/physics, and the
  wgpu renderer
- `wgpu` `30.0.2`

![iced window](docs/images/iced-window.png)

## What is verified

| Capability | Status |
|---|---|
| Model + textures load, correct colours | ✅ |
| Clipping masks (eyes, mouth, hair, body behind tail) | ✅ |
| Mipmapped textures — thin line art survives minification | ✅ (local patch) |
| Motion playback, looping, switching | ✅ `R` |
| Expressions (23 scanned from disk) | ✅ `E` |
| Physics (hair/tail follow) | ✅ |
| Gaze follows the cursor (head + eyes + torso) | ✅ |
| Window resize keeps framing | ✅ |
| Headless offscreen render to PNG | ✅ `examples/offscreen.rs` |
| Frame rate (release, 715 drawables) | ✅ ~50 fps, CPU-bound |
| Desktop-pet shell (transparent / on-top / click-through / tray) | ❌ not started |
| Breathing / eye blink / lip-sync | ❌ not started |

| iced window | offscreen render |
|---|---|
| ![window](docs/images/iced-window.png) | ![offscreen](docs/images/offscreen-frame.png) |

## Fidelity against the official viewer

Same model, both in a neutral pose: the official Cubism Viewer 5.3 on the left,
this renderer at `texture_scale = 1.0` on the right. Colours, mask edges, blend
modes and draw order match. The two captures are not frame-synchronised, so the
physics phase differs (mouth and hair sit a little differently).

![official viewer vs this renderer](docs/images/fidelity-vs-official.png)

Gaze tracking, cursor left vs right — head, eyes, shoulders and hair all move
because the driver writes the rig's *tracking inputs* and lets physics derive
`ParamAngle*`. It writes exactly the official viewer's five default bindings
(`AngleX`, `AngleY`, `ParamBodyAngleX`, `ParamEyeBallX`, `ParamEyeBallY`) and
nothing else; see `Live2dScene::set_gaze`.

![gaze](docs/images/gaze-compare.png)

## Requirements

- Rust **1.93+** (iced master's MSRV; edition 2024)
- A GPU with Vulkan/DX12/Metal for `wgpu`
- A Live2D model directory (`*.model3.json` + `*.moc3` + textures). **None is
  committed** — model assets are copyrighted.

## Quick start

```bash
# one-time: vendor the patched mocari (see "The two patches" below)
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/vendor-mocari.ps1

# open the default rig
cargo run --release -p app

# open any model by name, or by path
cargo run --release -p app -- --model Hiyori
cargo run --release -p app -- "path/to/model/xxx.model3.json"

# what can be named
cargo run --release -p app -- --list
```

`--model <name>` matches a model directory or file stem, case-insensitively,
inside `models/`, `.refs/models/`,
`.refs/cubism-web-samples/Samples/Resources/` and the rig under development.
`--track viewer` switches to the Cubism Viewer's own bindings (see
[the tracking section](#capturing-a-frame-without-a-window)). `--help` lists
everything.

Use `--release`. A debug build runs at ~5 fps because the per-frame CPU mesh work
for hundreds of drawables is unoptimised, which also makes animation look like
slow motion (the scene drops time it cannot catch up on).

**Controls:** `R` next motion clip · `E` next expression · move the mouse over
the window to make the model follow it.

## Frame pacing

`Live2dScene::advance` takes whatever interval the host measured and spends it in
fixed 1/60 s steps, keeping the rest for the next call. The pose is therefore a
function of elapsed time alone, not of how the host sliced it.

A host can hand the scene a **zero-length** interval, and a desktop window does:
iced delivers roughly 2–10% of its `window::frames()` messages with an instant
that equals the previous one, so `advance` gets `delta == 0` several times per
second. Treating such a frame as a real one is what used to make parts flicker —
`advance` reset every parameter to its default and then `apply_physics(0.0)`
returned before writing the physics outputs, so for one frame the hair, skirt and
accessories snapped to their rest pose and back. Only the parts the cursor does
not drive and physics does are affected, which is why it looked like isolated
components blinking. Under `--model 大肥鱼` the same bug is invisible in practice:
that rig takes ~28 ms per frame, so iced almost never produces the duplicate
instant for it.

`LIVE2D_PROFILE=1` prints the diagnosis once per 60 frames:

```
live2d profile: 60 frames 1.02 steps, rebuild 0.11 ms, upload 0.04 ms (mesh 0.03 over 84 drawables, clip 0.01), delta 16.67..16.7 ms (mean 16.67, 0 zero, 0 under 1 ms)
```

`steps` above 1 means the host presents slower than 60 Hz and the scene is
catching up; `zero` counts the zero-length intervals, which now cost nothing.
`drawables` is how many drawables actually re-uploaded — on a rig with dynamic
masks it tells a "one buffer was rebuilt" frame from a normal one.

## Framing

The model is framed by the **canvas** rectangle in the `.moc3` header, the same
box Cubism Editor and the Viewer use, not by the bounding box of the artwork. A
model whose art leaves spare room in its canvas therefore looks smaller and
off-centre in every host, ours included — `Rice` has a 1.5:1 landscape canvas
with the art filling 66% x 85% of it and sitting 6% right and 6% below the canvas
centre, so in a square window it occupies 61% x 53% of the width and height
where `Haru` occupies 40% x 83%. `cargo run --release -p live2d-render --example
inspect -- <model>` prints those numbers under `== framing ==`, so a framing
complaint can be answered with data instead of a guess.

## Capturing a frame without a window

`examples/offscreen.rs` renders into a texture and writes a PNG. Every piece of
state is an environment variable, so a capture is reproducible from the command
line alone:

```bash
cargo run --release -p live2d-render --example offscreen -- \
  "path/to/model/xxx.model3.json" frame.png 900 30
```

| Variable | Meaning |
|---|---|
| `LIVE2D_TEXTURE_SCALE` | upload resolution multiplier, default `1.0` |
| `LIVE2D_MASK` | mask render-target edge in pixels, default `1024` |
| `LIVE2D_MOTION` / `LIVE2D_EXPR` | motion / expression index to apply before the capture |
| `LIVE2D_GAZE` | `x,y` in `-1.0..=1.0`, applied as the cursor position |
| `LIVE2D_CLEAR_EXPR`, `LIVE2D_NO_MOTION`, `LIVE2D_NO_PHYSICS`, `LIVE2D_RECLIP` | flags — active when the variable is present, whatever its value |
| `LIVE2D_PROFILE` | flag — prints per-frame step count, mesh rebuild and upload cost, and frame-interval statistics |

`sweep` and `jolt` take their own knobs, documented in each file's header:

| Variable | Used by | Meaning |
|---|---|---|
| `SWEEP_DIRECT`, `SWEEP_RANGE`, `SWEEP_ALL`, `SWEEP_JUMP`, `SWEEP_SETTLE` | `sweep` | drive one parameter instead of the gaze, input range, watch every parameter, jump threshold, settle frames |
| `JOLT_MOTION`, `JOLT_RANGE`, `JOLT_SIZE`, `JOLT_SETTLE` | `jolt` | keep the idle motion, input range, capture size, settle frames |

`LIVE2D_TRACK` picks how the cursor reaches the model, for the app and every
example:

- `adapted` (default) — resolve what the rig was authored for. A rig built for
  VTube Studio takes head rotation on `AngleX`/`AngleY` and body motion on
  `PositionX`/`PositionY`; physics then derives `ParamAngle*` and
  `ParamBodyAngle*`. Measured on the rig in this repo: 11 593 changed pixels per
  frame across a full sweep (head, torso, hair, tail).
- `viewer` — reproduce the Cubism Viewer's own mouse bindings
  (`ParamAngleX`/`ParamAngleY`/`ParamBodyAngleX` + the eyes). Those are *physics
  outputs*, so physics overwrites them and only the eyes respond: 20 changed
  pixels per frame. Use this when comparing frames against the Viewer.

`examples/rig` prints both tables for a model in a second, without a GPU:

```
$ cargo run -p live2d-render --example rig -- "…/大肥鱼.model3.json"
== adapted ==
  head-yaw    -> AngleX             through-physics  (physics-input)
  body-yaw    -> PositionX          through-physics  (physics-input)
  eye-x       -> ParamEyeBallX      direct           (convention)
== viewer ==
  head-yaw    -> ParamAngleX        direct           (viewer-default)
```

## Other examples

| Example | Answers |
|---|---|
| `dump` | what is in the `.moc3`: drawables, mask sets, canvas, motion groups |
| `inspect` | what is in the *files*: physics settings, motion curves, expression targets, and how the art sits inside the canvas |
| `probe <model> <x> <y>` | which drawables cover a model-space point, in paint order |
| `switch <model> <Param> <v0> <v1>` | which drawables a parameter flips, and their metadata |
| `sweep` | what physics does along a horizontal gaze sweep, at 60 fps |
| `jolt` | how many pixels change per frame during a slow sweep, and where |
| `rig` | how cursor tracking is routed for a model, in both modes — no GPU needed |
| `offscreen` | one frame to a PNG, no window |

`sweep` is the fastest way to check a tracking bug: it prints the input parameter
and the physics outputs per frame, and flags any frame-to-frame jump. Prefix it
with `SWEEP_DIRECT=<ParamId>` to write that parameter directly instead of driving
the gaze.

`inspect` takes `LIVE2D_PARAMS=<substring>[,<substring>...]` to dump the matching
parameters with their ranges and current values, which is how you confirm a
parameter exists at all before blaming the renderer.

## Memory

The rig's five atlases are 4096x8192, 4096x8192, 2048x2048, **8192x8192** and
4096x4096 — 592 MB uncompressed. `load_model_runtime` keeps the decoded RGBA
alive inside `RuntimeModel`, so holding onto it costs CPU *and* GPU copies
(measured 1 565 MB). The scene clones the runtime, drops the assets, and uploads
at half resolution by default:

| | working set |
|---|---|
| naive (keeps decoded textures, full res) | 1 565 MB |
| assets dropped, full res | ~900 MB |
| assets dropped, `texture_scale = 0.5` (default) | **462 MB** |
| the above + mip chain (+33% VRAM) | ~488 MB |

Pass `SceneOptions { texture_scale: 1.0, ..Default::default() }` for full quality.

## Layout

```
crates/live2d-render/          engine-agnostic: depends on wgpu + mocari only
  src/bounds.rs                canvas info -> view-fit matrix
  src/scene.rs                 Live2dScene: load, advance, encode mask+model passes
  src/error.rs                 error taxonomy
  examples/*.rs                the diagnostics listed above
  examples/shared/gpu.rs       headless GPU bootstrap shared by the examples
app/                           the iced shell
  src/main.rs                  application, subscription, keyboard
  src/live2d.rs                Program + Primitive + Pipeline
patches/                       the four mocari fixes, as patch files
scripts/vendor-mocari.ps1      rebuilds .refs/mocari from the registry + patches
scripts/shot.ps1               DPI-aware window screenshot (dev tool)
scripts/shot-window.ps1        screenshot any window by title prefix (dev tool)
docs/                          feasibility write-up, plan, defect records
```

The split matters: keeping `live2d-render` free of iced means a `wgpu`/`iced`
upgrade touches one layer, and the renderer stays testable headlessly.

## The four patches

All four live in `patches/` and are applied to a vendored copy at `.refs/mocari`
(wired up through `[patch.crates-io]` in `Cargo.toml`); `scripts/vendor-mocari.ps1`
rebuilds that copy from the cargo registry.

**1. Negative mask sentinel.** `mocari 0.4.1` cannot open this model: one of its
715 drawables carries `masks == [-1]`, a sentinel some exporters write for "not
masked", and mocari feeds it straight into a `usize` conversion:

```
failed to create a clipping layout: invalid mask drawable index -1
```

Filtering negative mask entries treats that drawable as unmasked, which is the
only sensible reading. The official viewer reports 181 clipped drawables against
our 182 raw entries, which confirms the semantics.
See [`patches/mocari-0.4.1-negative-masks.patch`](patches/mocari-0.4.1-negative-masks.patch).

**2. Missing mip chain.** mocari uploads every texture with `mip_level_count: 1`,
so a rig authored at 4096–8192 px and drawn a few hundred pixels tall minifies
4–8x with a single bilinear tap: line art breaks into dotted speckles. The
official viewer enables mipmaps by default. The patch builds the chain on upload
and sets `mipmap_filter = Linear`; it costs +33% VRAM and ~0.5 s of startup.
See [`patches/mocari-0.4.1-mipmaps.patch`](patches/mocari-0.4.1-mipmaps.patch).

**3. Physics chains do not propagate.** Each physics setting writes its output back into the
parameter cache, and that cache is how a later setting sees the result in the same
substep — the official framework says so in a comment on the cache. mocari rebinds the
cache slice and passes it as the computed value, so the write is a no-op. On this rig
**113 reads** across settings depend on that propagation (hair, tail, ribbons, chest,
skirt), so before the patch everything past the first hop stood still. The fix is two
lines; `FrontHairX1` goes from 0.000 to -0.136 while `ParamAngleX` sweeps to +26.5.
See [`patches/mocari-0.4.1-chained-physics.patch`](patches/mocari-0.4.1-chained-physics.patch)
and [`docs/mocari-chained-physics.md`](docs/mocari-chained-physics.md).

**4. Geometry is uploaded one drawable at a time, and dynamic masks are fatal.**
Two problems in the same file, and together they are why the 715-drawable rig ran
at 35 fps. Every drawable owned its own vertex buffer, so a frame that deforms the
whole model issued 715 `write_buffer` calls. Worse, `update_drawables` returned an
error when a drawable's mask set changed — and drawable 373 of this rig switches
masks on **every frame**, because that is what dynamic masks are for. The caller's
only option was to rebuild every GPU buffer, which is what cost 18 ms of upload
time per frame. Neither masks, textures nor blend modes live in the vertex buffer:
they belong to the draw loop and the clipping plan. The patch packs every drawable
into one shared vertex buffer (one write per frame), applies those four values in
place, and reports them through `WgpuMeshUpdate::structure_changed()` so the caller
rebuilds clipping resources and nothing else. Upload time on this rig went from
**18 ms to 1.2 ms per frame**, the scene went from 1.6 animation steps per frame
(35 fps) to 1.00 (60 fps), and offscreen captures of both this rig and `Rice` are
pixel-identical to the unpatched build.
See [`patches/mocari-0.4.1-batched-vertex-upload.patch`](patches/mocari-0.4.1-batched-vertex-upload.patch).

All four belong upstream — they should not be every consumer's problem.

## Why the git dependency on iced

`mocari`'s wgpu backend needs `wgpu 30`, which is what iced **master** uses
(`0.14.0` is still on `wgpu 27`). Pinning master also pins the whole `wgpu`
major, so `WgpuLive2dRenderer` links straight into `Pipeline`. It is pinned by
`rev`, and iced pins its own git dependencies by `rev` too, so builds are
reproducible. When 0.15 ships, switch to the crates.io version — the glue should
not need changes.

## Things worth knowing before extending this

- **mocari's `*_normalized` setters take `0.0..=1.0`, not `-1.0..=1.0`.** They
  clamp silently, so a Cubism-style offset pins every negative value to the
  parameter minimum: the model freezes until the cursor crosses its centre line
  and then snaps into motion. `set_tracking_input` in `scene.rs` converts once;
  go through it rather than calling mocari directly.
- **Framing must come from the `.moc3` canvas**, not from a bounding box of all
  drawables. This model has 715 drawables including hidden spares (props, spare
  hands, alternate body types); bounding-box centring visibly offsets the
  character. See `ModelBounds::from_canvas`.
- **`model3.json` is not always complete.** Here it declares zero motions and
  zero expressions while the files sit in `Animation/` and `Exp3/`. `Live2dScene`
  scans the model directory as a fallback and plays `IDLE*` first.
- **Colour space:** the `format` handed to `Pipeline::new` was `Bgra8Unorm`
  (non-sRGB), which matches mocari's gamma-space blending. If a platform gives
  you `*UnormSrgb`, you must render into your own `Rgba8Unorm` target and do
  gamma→linear when compositing, or the model washes out. The startup line
  prints the format it got.
- **`Primitive::draw` must return `false`** and the work must happen in
  `Primitive::render`, because the mask pass has to be inserted between the
  passes iced controls.
- **Textures need mipmaps** — see patch 2 above.
- **Mouse tracking is resolved per model, not hardcoded.** `live2d-render/src/rig.rs`
  maps six slots (head yaw/pitch, body yaw/lean, eyes) onto whatever the rig
  exposes. A rig built for VTube Studio takes `AngleX`/`AngleY` and
  `PositionX`/`PositionY` because its `ParamAngle*` parameters are physics
  outputs; a classic rig takes the `Param*` names directly. The choice is decided
  from `physics3.json`, printed at load and inspectable with `examples/rig`.
  `LIVE2D_TRACK=viewer` forces the Cubism Viewer's own binding set instead.

## Troubleshooting (this machine only)

If `cargo build` fails with `schannel: AcquireCredentialsHandle failed`
(`SEC_E_NO_CREDENTIALS`), the process is sandboxed: Windows TLS refuses to hand
out credentials to restricted processes, so **every** HTTPS client (curl, git,
cargo) fails while plain TCP still connects. Run the build outside the sandbox —
this is not a project problem.

## Docs

- [`docs/rust-iced-live2d-plan.md`](docs/rust-iced-live2d-plan.md) — verified
  architecture, iced 0.15-dev API notes, pitfalls, calibrated effort estimate
- [`docs/test-models.md`](docs/test-models.md) — the model corpus, where each
  model's terms of use live, and what each rig exercises
- [`docs/mocari-chained-physics.md`](docs/mocari-chained-physics.md) — confirmed
  mocari defect: physics outputs never reached the parameter cache, so the chained
  half of the rig stood still
- [`docs/live2d-native-feasibility.md`](docs/live2d-native-feasibility.md) —
  broader survey of Live2D on native platforms (platform matrix, licensing,
  difficulty ladder)
