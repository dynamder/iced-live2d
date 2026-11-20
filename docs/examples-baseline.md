# Examples rendering baseline

A pixel-level oracle for the refactor in `docs/change-plan.md`: every example's
scene is captured before the changes and must be captured identically after them.
No library change in that plan is allowed to move these frames.

Recorded at commit `0af66f2` on the llvmpipe adapter. Delete this file once the
refactor has landed and the captures are part of the normal review.

## Reproducing

```bash
cargo build --release -p live2d-render --example offscreen
scripts/examples-baseline.sh .refs/models target/examples-baseline
```

The script prints one `sha256 file` line per recipe. Compare with the tables
below; a mismatch is a rendering change unless the recipe itself changed.

## Why `offscreen`

The `iced-live2d` examples and the app open a window and step by wall clock, so
two runs of the same binary do not produce byte-identical frames. They draw
through the same `Session`, `View` and `ModelGpu` as `offscreen`, so each recipe
below reproduces one example's scene — model, texture budget, mask budget, fill
and the motion it starts on load — and advances it by a fixed number of `STEP`s.
That is deterministic: two consecutive runs produce the same bytes.

`offscreen` always sets `scan_sidecars`; the recipes that mirror an example with
it off still play group 0 clip 0 of the model's own groups, so the scan does not
change the frame.

## Recipes and their hashes

| Example | Model | `LIVE2D_TEXTURE_SCALE` | `LIVE2D_MASK` | `LIVE2D_FILL` | `LIVE2D_MOTION` | Size | Steps | sha256 |
|---|---|---|---|---|---|---|---|---|
| `app` | Hiyori | 0.5 | 1024 | 0.925 | 0,0 | 640 | 30 | `51b0896806a8125947a85441ae5abfdd2bfa033657ddc3a7d5e85e1a87e21af0` |
| `iced-live2d --example minimal` | Hiyori | 0.5 | 1024 | 0.925 | 0,0 | 640 | 30 | `51b0896806a8125947a85441ae5abfdd2bfa033657ddc3a7d5e85e1a87e21af0` |
| `iced-live2d --example interact` | Hiyori | 0.5 | 1024 | 0.925 | 0,0 | 640 | 30 | `51b0896806a8125947a85441ae5abfdd2bfa033657ddc3a7d5e85e1a87e21af0` |
| `iced-live2d --example policy` | Hiyori | 0.5 | 1024 | 0.925 | 0,0 | 640 | 30 | `51b0896806a8125947a85441ae5abfdd2bfa033657ddc3a7d5e85e1a87e21af0` |
| `iced-live2d --example typed` | Wanko | 0.5 | 1024 | 0.925 | 0,0 | 640 | 30 | `70406b56cb05a6f86a7adec7121e818525c74b48d0c0ca8345399d093ec4c685` |
| `live2d-render --example offscreen` | Hiyori | 1.0 | 1024 | 0.925 | — | 900 | 30 | `54a98309e3bf735cf90466482e1e0c9f0bc518d26c5538563a323beb11992b56` |

The four Hiyori scenes share a hash because they differ only in host-side policy
and pose drivers; each row stays separate so a changed asset or model budget in
one example is still visible.

## Text observables from examples that render internally

Both write a frame-difference trace over a fixed look-at sweep, so they catch
renderer changes that a single still frame misses:

| Example | Model | sha256 of stdout |
|---|---|---|
| `live2d-render --example jolt` | Hiyori, default knobs | `d076ba3b08bb7f91712b67d8671d1c9d4b1e65518eaecbbda5580d628efceb5a` |
| `live2d-render --example sweep` | Hiyori, default knobs | `bd70a80d07d8418e3dfefe4db0cbe727a32ea8f561d396f5b33a34faf26e3080` |

## Examples with no rendering state

`dump`, `inspect`, `probe`, `rig`, `switch` and `live2d-render --example policy`
read model data or step parameters and print text; they never touch the GPU.
