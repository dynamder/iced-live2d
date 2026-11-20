# Gaps

What this component does not do yet, read off the code rather than the wish
list. Every entry says where the gap lives and what closing it costs:

- **wire** — `mocari` or the render layer already has the capability and only
  the plumbing is missing;
- **build** — nothing below it has the capability, so it is new code (or a
  shader).

Delete an entry when it lands. The README links here for the conclusion; this
file is the working list.

## Must fix

Everything below stops a host from doing something its users expect, and none of
it can be worked around from the widget's public API.

### Host parameter read-back

`parameters().get` returns the write the host set, not the value the model shows:
motion, expressions, look-at, physics and pose all contribute around it. A host
that wants to add to the mouth's current opening, or a UI that shows a slider's
true position, has nothing to read. The render layer's
`parameter_value_by_index` is diagnostics-only and lives behind the pipeline.

**build**, medium: a snapshot the pipeline fills after the step and publishes one
frame late, plus a way to say which parameters matter.

### Loading is synchronous and terminal

`Asset::parse` and the texture upload run inside `Pipeline::prepare`
([pipeline.rs](../crates/iced-live2d/src/pipeline.rs#L426-L439)), so the first
frame blocks for the whole load: there is no progress and no loading state. A
failed parse is cached per `AssetId`
([pipeline.rs](../crates/iced-live2d/src/pipeline.rs#L531-L541)), so retrying
requires a brand-new `Live2dAsset` — a path typo or one unlucky read is a
permanent black rectangle, reported only if the host subscribed.

**build**, medium.

### The frame loop never idles

The widget asks for the next frame whenever the model is not paused
([widget.rs](../crates/iced-live2d/src/widget.rs#L934-L938)) without asking
whether anything moved; `is_animating`
([model.rs](../crates/live2d-render/src/model.rs#L322-L327)) lives in the render
layer and is private. A model with no motion, no expressions and no physics still
redraws at 60 Hz, and a desktop pet is exactly a window that is idle most of the
time.

**wire**, small.

### Texture scale is fixed at construction

`texture_scale` is an `AssetOptions` constant
([asset.rs](../crates/live2d-render/src/asset.rs#L36-L48)) while the render target
is measured in physical pixels, so a HiDPI window stretches a half-size atlas and
the only remedy is rebuilding the asset. There is no runtime re-scale and no
DPI-aware choice, although a 4K rig is the difference between 37 MB and 592 MB of
VRAM.

**build**, medium.

## Should have

- **Provider transitions.** A host layer appears and disappears instantly, and the
  fixed step order cannot reproduce VTS's priority model: there, a one-shot clip
  (P3) overrides tracking (P2) and an expression (P4) overrides both, with a smooth
  fade between providers. More blend modes do not close this; a per-parameter
  provider state does.
- **Global opacity.** The model has per-part opacity only, and parts do not cover
  every drawable, so fading a whole model in or out is not expressible.
- **Non-rectangular clipping.** The primitive receives an axis-aligned
  `clip_bounds` ([pipeline.rs](../crates/iced-live2d/src/pipeline.rs#L350-L355)),
  so a rounded container does not clip the model.
- **View transforms.** `View` knows aspect and `fill` only
  ([view.rs](../crates/live2d-render/src/view.rs#L40-L143)): no pan, zoom,
  rotation, mirror or anchor.
- **Model-target curves.** A `motion3.json` curve can target the model instead of a
  parameter: `Opacity` fades the whole model, `EyeBlink` multiplies the model's
  eye-blink group, `LipSync` adds to its lip-sync group. `mocari` drops all three
  ([motion.rs](../vendor/mocari/src/motion.rs#L144-L162)). Mark animates its blinks
  this way in every clip, so those blinks are lost today; Haru ships an `Opacity`
  curve per clip that happens to stay at 1.0. The first two need the
  `model3.json` `Groups` ids plumbed in, and `Opacity` needs a renderer change.
- **Playback control.** No seek, speed or `time_scale`; the loop flag is the
  file's; `stop_motion` has no fade-out; `ModelInfo` reports clip counts without
  names or durations ([info.rs](../crates/live2d-render/src/info.rs#L35-L42));
  `Model::current_motion` is not exposed through the widget.
- **Physics beyond a switch.** `mocari` has gravity, wind and reset; the render
  layer exposes one boolean ([model.rs](../crates/live2d-render/src/model.rs#L199-L211)).
  Pose playback has no toggle either.
- **Asset sources.** `Live2dAsset::new` takes a path
  ([model.rs](../crates/iced-live2d/src/model.rs#L46)); a self-contained binary
  must extract its model to disk first.
- **Model swapping** as a first-class operation, instead of building a new model
  object.
- **Pause semantics.** `pause` is a total freeze — no steps, no deform, no upload
  — so a paused model cannot be posed or re-framed, and physics cannot be left
  running while the motion stops.
- **Display names.** The `cdi3.json` parser exists in `mocari` and nothing loads
  it, so UI can only show `ParamAngleX`-style ids.

## Deliberately not here

- Keyboard input, focus and accessibility: a character canvas has little to say.
- Pinch and pen pressure: touch and tablet surfaces.
- wasm and the tiny-skia backend: the renderer is wgpu only.
- A desktop-pet shell (transparent, on-top, click-through): the host's job.
- Motion priority, queues and interruption rules: policy belongs to the host once
  the render layer can blend.

## Adoption

The crate pins iced master by `rev` and sets `publish = false`. Until iced 0.15
ships with the `wgpu` major `mocari` needs, no ordinary host can depend on a
release, which is a bigger obstacle than anything above.
