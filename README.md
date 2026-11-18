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

Same model, same rest pose: the official Cubism Viewer 5.3 on the left, this
renderer at `texture_scale = 1.0` on the right. Colours, mask edges, blend modes
and draw order match; only the physics phase differs, because the two were
captured at different moments.

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

# run with any model
cargo run --release -p app -- "path/to/model/xxx.model3.json"
```

Use `--release`. A debug build runs at ~5 fps because the per-frame CPU mesh work
for hundreds of drawables is unoptimised, which also makes animation look like
slow motion (each frame's delta is clamped to 0.1 s).

**Controls:** `R` next motion clip · `E` next expression · move the mouse over
the window to make the model follow it.

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
| `LIVE2D_PROFILE` | flag — prints per-frame CPU and upload cost |

## Other examples

| Example | Answers |
|---|---|
| `dump` | what is in the `.moc3`: drawables, mask sets, canvas, motion groups |
| `inspect` | what is in the *files*: physics settings, motion curves, expression targets |
| `probe <model> <x> <y>` | which drawables cover a model-space point, in paint order |
| `switch <model> <Param> <v0> <v1>` | which drawables a parameter flips, and their metadata |
| `sweep` | what physics does along a horizontal gaze sweep, at 60 fps |
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
patches/                       the two mocari fixes, as patch files
scripts/vendor-mocari.ps1      rebuilds .refs/mocari from the registry + patches
scripts/shot.ps1               DPI-aware window screenshot (dev tool)
scripts/shot-window.ps1        screenshot any window by title prefix (dev tool)
docs/                          feasibility write-up, plan, defect records
```

The split matters: keeping `live2d-render` free of iced means a `wgpu`/`iced`
upgrade touches one layer, and the renderer stays testable headlessly.

## The two patches

Both live in `patches/` and are applied to a vendored copy at `.refs/mocari`
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

Both belong upstream — they should not be every consumer's problem.

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

## Troubleshooting (this machine only)

If `cargo build` fails with `schannel: AcquireCredentialsHandle failed`
(`SEC_E_NO_CREDENTIALS`), the process is sandboxed: Windows TLS refuses to hand
out credentials to restricted processes, so **every** HTTPS client (curl, git,
cargo) fails while plain TCP still connects. Run the build outside the sandbox —
this is not a project problem.

## Docs

- [`docs/rust-iced-live2d-plan.md`](docs/rust-iced-live2d-plan.md) — verified
  architecture, iced 0.15-dev API notes, pitfalls, calibrated effort estimate
- [`docs/mocari-suspected-physics-chaining.md`](docs/mocari-suspected-physics-chaining.md)
  — open suspicion: physics outputs may not reach downstream settings
- [`docs/live2d-native-feasibility.md`](docs/live2d-native-feasibility.md) —
  broader survey of Live2D on native platforms (platform matrix, licensing,
  difficulty ladder)
