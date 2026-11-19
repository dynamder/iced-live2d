# Webview + Cubism Web SDK vs native iced: what each costs in RAM

The question this answers: if the rig were hosted in a Chromium webview running
the **official Cubism Web SDK**, how much memory would it take next to the
native `iced` + `mocari` app that already works?

Everything below is measured, not estimated. The point is not a spec sheet — it
is whether "webview is fine" survives contact with a 592 MB model.

## Setup

| | native | web |
|---|---|---|
| host | `target/release/app.exe` | Edge 157 `--app` with a throwaway `--user-data-dir` |
| drawable | 640×640 window | 640 CSS px canvas |
| device pixels | 960×960 (150% DPI) | 960×960 (`devicePixelRatio` honoured) |
| model files | `.refs/cubism-web-samples/Samples/Resources/Hiyori`, `DeepSeek-fish-live2d/DeepSeek` | same directories, same bytes |
| VRAM | not counted | not counted |

Edge `--app` is the same Chromium engine and the same multi-process layout as
the WebView2 runtime a Tauri app embeds, plus Edge's own satellite processes, so
the web column is an upper bound rather than a best case.

Metric is **private working set** — what Task Manager shows as "Memory" — summed
over the whole process tree (browser, GPU, renderer, network, storage, crashpad).
Working set and private commit are listed too because they answer different
questions: working set includes DLL pages shared with every other Chromium app,
commit is what the process has reserved. Samples are taken 2–3 s apart after
warm-up; the median is the number.

## Results

| case | private working set | working set | private commit | processes |
|---|---|---|---|---|
| native + Hiyori | **118 MB** | 221 MB | 182 MB | 2 (`app` 116 + `conhost` 2) |
| webview + Hiyori | **309 MB** | 782 MB | 434 MB | 12 |
| native + 大肥鱼 (`texture_scale = 0.5`) | **388 MB** | 491 MB | 455 MB | 2 |
| webview + 大肥鱼 | **1 252 MB** | 2 348 MB | 1 334 MB | 8 |
| webview, blank page | **193 MB** | 614 MB | 300 MB | 12 |

Two ratios, both on the same content: **2.6× for a 2-texture sample model, 3.2×
for the real rig**. A blank webview alone costs 1.6× the entire native app with
Hiyori loaded.

Sample-to-sample spread was ±5% on the web rows (Chromium sheds idle helpers
over the first ~20 s) and ±1 MB on the native rows.

Two follow-ups worth recording. First, the rig row is 491 MB of working set
here, while the README's memory table says 462 MB: same metric, earlier
measurement, and the cause is not pinned down — working set counts pages shared
with the GPU driver, so adapter/driver state moves it. Private working set, the
metric that does not include shared pages, is stable at 388 MB. Second, the Rust
toolchain was ruled out by A/B: the same sources built with 1.97.1 and 1.100.0
land at 388.3 MB / 491.8 MB and 388.4 MB / 491.6 MB, i.e. inside each other's
noise.

## Where the bytes go

`193 MB` is the floor before a single texel is drawn — Chromium's browser
process, GPU process, one renderer, network/storage/audio services, crashpad. No
amount of GPU work makes that cheaper.

The per-model delta is where the design difference shows:

| | native delta | web delta |
|---|---|---|
| Hiyori (2×2048² textures) | — | +115 MB |
| 大肥鱼 (5 textures, 155 Mpx) | — | +1 059 MB |

The web side keeps **decoded bitmaps alive**: `LappTextureManager` retains the
`Image` element it uploaded from, so a 155 Mpx atlas costs 620 MB of RGBA in the
renderer *in addition to* the GPU texture. The native renderer uploads and drops
the decoded pixels, and `texture_scale = 0.5` halves the upload — that is why the
fish starts at 388 MB instead of 1.2 GB.

For 大肥鱼 the web renderer process alone is **1 047 MB**, plus 106 MB in the GPU
process. Nothing there is a leak; it is what the model weighs when every texture
is decoded at full size and kept.

## Two traps that cost more time than the measurement

### Clip mask render textures are capped and the failure is silent

`CubismClippingManager_WebGL` packs clipping masks into render targets:

```ts
// cubismclippingmanager.ts:17-18
const ClippingMaskMaxCountOnDefault = 36;             // single render texture
const ClippingMaskMaxCountOnMultiRenderTexture = 32;  // per texture, 2+ textures
```

The renderer defaults to `initialize(model, maskBufferCount = 1)`
(`cubismrenderer_webgl.ts:586`), so a model with more than 36 clipping groups
hits `setupLayoutBounds`'s early return: it logs `not supported mask count` to
the console and keeps drawing **with the previous mask bound**. The result is
not a crash and not a blank screen — it is a model whose mouth and blush
artmeshes are painted at full size across the face.

大肥鱼 has more than 36 groups. `createRenderer(4)` (128 groups of capacity)
renders it identically to `mocari`. There is no API to ask how many groups a
model needs, and the official demo passes no argument either, so the demo is
broken on this rig too.

`mocari` has no equivalent limit: it renders one clipping mask per drawable pass
instead of packing groups, which is slower per mask but has no cap.

### The canvas buffer has to be sized in device pixels

A `<canvas width="640" height="640">` styled at 640 CSS px is a 640×640 WebGL
buffer stretched to 960×960 device pixels at 150% DPI: soft, and free of charge.
The rig looked "lower resolution in the webview" until `canvas.width =
640 * devicePixelRatio`. The SDK has nothing to do with it — the host page does.

## Limits of this measurement

- Edge `--app`, not a real Tauri/WebView2 host. Expect fewer satellite
  processes; the renderer and GPU heaps are the same engine and do not improve.
- Electron was not measured; it is Chromium plus a Node runtime on top, so it
  starts above this number.
- GPU/VRAM is excluded on both sides. WebGL textures live in driver memory that
  Task Manager does not attribute to the process.
- Both hosts render 60 fps; frame pacing was not part of this benchmark.

## Reproducing

```powershell
# official SDK (not vendored, license): unpack to .refs/cubism-web-sdk
node -e "require('https').get('https://cubism.live2d.com/sdk-web/bin/CubismSdkForWeb-5-r.4.zip',r=>r.pipe(require('fs').createWriteStream('.refs/cubism-web-sdk.zip')))"
powershell -NoProfile -Command "Expand-Archive .refs/cubism-web-sdk.zip .refs/cubism-web-sdk"

# probe page: esbuild on PATH + the SDK framework sources (-Sdk / -Esbuild override)
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/web-probe/build.ps1

# one case per run; -Shot writes a window capture
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/memory-probe.ps1 -Kind web
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/memory-probe.ps1 -Kind blank
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/memory-probe.ps1 -Kind native -Model Hiyori
```

The rig needs the mask fix to render at all:

```
http://127.0.0.1:8731/web/index.html?dir=/rig/DeepSeek/&json=%E5%A4%A7%E8%82%A5%E9%B1%BC.model3.json&maskrt=4
```

`?debug=1` prints the model canvas, drawable count, mask buffer size and mask
render texture count over the canvas, which is how the quota trap was found.
