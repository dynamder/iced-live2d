# iced-live2d

Live2D Cubism models as an [iced](https://github.com/iced-rs/iced) widget, in pure
Rust: `.moc3` parsing, motion/expression/physics playback and wgpu rendering, with no
Cubism Core binary and no C++ toolchain in the build.

![iced window](docs/images/iced-window.png)

*Rendered from `DeepSeek-chan（大肥鱼）`, a free model drawn and separated by 神宫凉子
and rigged by 霓虹猫_NeonNeko. See [Model credits](#model-credits).*

The widget owns the parts that are tedious to get right — texture budgets, GPU state,
frame pacing, look-at routing, hit testing, event delivery — so a host writes an
asset, a model, and a `view`:

```rust
live2d(&app.hiyori)
    .gestures(Gestures::default())
    .on_click(|tap| Message::Pet { model: tap.model, area: tap.area })
    .look_at(look_at::cursor)
```

## Status

Pre-0.1: extracted from a working desktop-pet application, so the API is what that
application needed and may still move. The renderer is **wgpu only** — the widget delegates to
iced's `shader` primitive, which the tiny-skia backend does not have.

| Capability | Status |
|---|---|
| model and textures, correct colours, clipping masks | ✅ |
| mipmapped textures — thin line art survives minification | ✅ |
| motions: groups, looping, cross-faded switching, expressions | ✅ |
| physics, including settings that chain into each other | ✅ |
| look-at: head, eyes, torso, routed per rig | ✅ |
| parameter writes staged before or after physics — overwrite, add or multiply | ✅ |
| part overrides, replayed every step | ✅ |
| hit areas; hover, press, drag, wheel | ✅ raw or classified |
| typed vocabularies, validated at load | ✅ |
| headless offscreen render to PNG | ✅ |
| 60 fps on the 715-drawable reference rig | ✅ |

Not there yet: reading back the effective parameter values, smooth transitions
between the clip and a host layer, asynchronous loading, idle frames, DPI-aware
texture budgets, global opacity, non-rectangular clipping, view transforms
(pan/zoom of the framing itself), playback control (seek, speed), physics beyond a
switch, loading a model from bytes, and a pause that is more than a total freeze.
[docs/gaps.md](docs/gaps.md) tracks each of them with where it lives and what
closing it costs.

Deliberately out of scope: keyboard and focus, accessibility, pinch, pen pressure,
inertia and physics drags, breathing/eye-blink/lip-sync helpers, a desktop-pet
shell (transparent, on-top, click-through), motion priority policy, and the
tiny-skia and wasm targets.

## Fidelity

Same model, both in a neutral pose: the official Cubism Viewer 5.3 on the left, this
renderer at `texture_scale = 1.0` on the right. Colours, mask edges, blend modes and
draw order match. The captures are not frame-synchronised, so the physics phase differs
(mouth and hair sit a little differently).

![official viewer vs this renderer](docs/images/fidelity-vs-official.png)

## Requirements

- Rust **1.93+** (edition 2024)
- A GPU with Vulkan/DX12/Metal
- iced **master**, pinned by `rev` — the `shader` widget and the `advanced` feature
  the crate needs are not in a release yet
- A Live2D model directory (`*.model3.json` + `*.moc3` + textures). **No model is
  part of this repository**; [docs/test-models.md](docs/test-models.md) lists where
  the official samples come from and what their terms are

## Getting started

The crate pins iced master, so the host has to add the same pin:

```toml
[dependencies]
iced = { git = "https://github.com/iced-rs/iced", rev = "e5c350f271a8046f875c6662d452fde9e462d486", features = ["advanced"] }
iced-live2d = { git = "https://github.com/dynamder/iced-live2d" }
```

A complete host is an asset, a model, and a `view`:

```rust
use iced_live2d::{
    AssetOptions, Live2dAsset, Live2dModel, ModelOptions, Raw, live2d, look_at,
};

struct App {
    hiyori: Live2dModel<Raw>,
}

fn new() -> App {
    let asset = Live2dAsset::new(
        "Hiyori.model3.json",
        AssetOptions { texture_scale: 0.5, scan_sidecars: true },
    );
    let hiyori = Live2dModel::<Raw>::new(&asset, ModelOptions::default());
    App { hiyori }
}

fn view(app: &App) -> impl iced::Widget<Message> {
    live2d(&app.hiyori)
        .on_event(Message::Live2d)
        .look_at(look_at::cursor)
}
```

The asset/model split is what makes several instances cheap: they share the textures
and the parsed model, and each keeps its own parameter values, playback position and
physics state.

Both constructors take their options explicitly, and there is no variant that invents
them: whether to spend 592 MB of VRAM on 4096px textures or 37 on half-size, whether a
folder of animation files is a loop set, and how host writes combine with what plays,
are not things a component can guess. `AssetOptions::default()` and
`ModelOptions::default()` are a starting point to override, not a hidden decision.

For a runnable host, this repository carries a demo app and three examples; all of
them need a model path:

```bash
cargo run --release -p app -- "path/to/Hiyori.model3.json"
cargo run --release -p app -- "path/to/Hiyori.model3.json" --track viewer
```

Use `--release`: a debug build runs at ~5 fps because the per-frame CPU mesh work for
hundreds of drawables is unoptimised, which also makes motion look like slow motion.

## Using the component

### Assets, models, views

| Type | Owns | Lifetime |
|---|---|---|
| `Live2dAsset` | file path, texture budget (`AssetOptions`) | as long as any model of it, plus itself |
| `Live2dModel<I>` | parameters, parts, playback, physics, budgets and write policy (`ModelOptions`) | the host's |
| `Live2d<'_, Message, I>` | framing, pointer, event channel | while it is in the UI tree |

Below the component the same split reappears as `live2d_render::{Asset, Model,
ModelGpu, View}`, and `Model` there is pure CPU: it can be stepped, hit-tested and read
back without a device.

### Declaring what a model must offer

A model is driven through a **vocabulary**: five lists of names the host declares in
enums and promises the model has. Declaring one is `impl Interface` plus the enums;
the derive (`--features macros`) writes the four vocabulary items:

```rust
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
#[vocabulary(prefix = "Param")]
enum FaceParameter { AngleX, AngleY, MouthOpenY }

struct Face;
impl Interface for Face {
    type Parameter = FaceParameter;
    // …Part, Motion, Expression, HitArea
}
let hiyori = Live2dModel::<Face>::new(&asset, ModelOptions::default());
```

- **Validation happens once, at load, before a texel is uploaded.** A model that lacks
  any declared name fails to load with the missing names listed
  (`LoadFailure::Interface`) instead of silently driving nothing.
- **Keys are typed.** A key can only be produced by the vocabulary that declares it,
  so `play_motion(FaceMotion::TapBody, 0)` cannot be called on a model whose interface
  has no `TapBody`.
- **`Raw` is the escape hatch.** `Live2dModel::<Raw>::new(&asset, options)` takes the
  model's own `ModelInfo` as the vocabulary; keys carry the asset they were minted
  from, so a key handed to a model of another asset is rejected and reported instead
  of landing on the same index.
- **An empty vocabulary says "not this interface".** An interface whose `Expression`
  enum has no variants can never be asked for an expression.

### Driving a model

```rust
model.parameters().insert(FaceParameter::MouthOpenY, 0.8);   // overwrite, the default
model.parameters().add(FaceParameter::AngleX, wave);         // add on top of what plays
model.parameters().multiply(FaceParameter::AngleY, 0.5);     // multiply what plays
model.parts().insert(FacePart::ArmA, 0.0);                   // 0.0..=1.0
model.play_motion(FaceMotion::TapBody, 0);                   // (group, clip), as Cubism indexes
model.play_random_motion(FaceMotion::Idle);
model.stop_motion();
```

Every write is **replayed on every step**, after look-at:

```text
reset → motion → expression → look-at → host (before physics)
      → parts → physics → host (after physics) → pose
```

`ModelOptions` carries the two decisions a host makes once:

```rust
let model = Live2dModel::<Face>::new(
    &asset,
    ModelOptions {
        mask_size: 1024,
        parameter_policy: ParameterPolicy {
            blend: ParameterBlend::Overwrite,
            stage: ParameterStage::Auto,
        },
        look_at_blend: LookAtBlend::Overwrite,
    },
);
```

- **`blend`** decides how a write combines with what played: `Overwrite` (the default),
  `Add` or `Multiply`. A write can name its own blend (`add`, `multiply`, `apply`) and
  then ignores the policy's blend; its stage still follows the policy unless it names
  one.
- **`stage`** decides where it lands. `Auto`, the default, reads the model's own
  `physics3.json`: a parameter physics writes is staged after physics, everything else
  before it. A write can name `BeforePhysics` or `AfterPhysics` explicitly.
- **`look_at_blend`** decides whether the built-in look-at replaces the pose
  (`Overwrite`, the default) or adds to it (`Add`).
- A **physics output** written before physics is overwritten; the component reports
  that once as a `Problem` instead of leaving a silent no-op.

The policy is live: `parameter_policy_mut()` and `look_at_blend_mut()` change it
without rebuilding the model. There is no constructor that invents it —
`ModelOptions::default()` is how a host explicitly chooses the defaults.

A key holds **one** write: `add` replaces an earlier `insert` on the same key, so a
host that wants to combine a tracking value and an effect computes the final value
itself.

Values are **raw**, in the range the model declares; `ModelInfo` carries
`minimum`/`maximum`/`default` per parameter. Clamping is the render layer's job.

**The library does not decide what plays.** There is no idle policy, no automatic
first motion, and no directory scan unless asked for: a model nobody tells to play
stays in its rest pose. "Idle" is the host's five lines:

```rust
// on load: start something
Live2dEvent::ModelLoaded { .. } => model.play_motion(FaceMotion::Idle, 0),
// and when a one-shot ends: start the next one
Live2dEvent::MotionFinishedPlaying { .. } => model.play_random_motion(FaceMotion::Idle),
```

That is deliberate. A component that picks for the host also picks *wrong*: this
repository's own demo once looped a rig's "switch to the hand holding a foxtail" clips
as if they were idle animations, because the library had invented a motion group from
a directory listing and then chosen from it at random.

### Events

- **Loading is deferred and reported.** `Live2dAsset::new` does no IO; the first frame
  that draws a model loads it, and the outcome arrives as
  `Live2dEvent::ModelLoaded { info }` or `ModelFailedToLoad { failure }`. A failed load
  is not retried, and the widget keeps reserving its space.
- **Events are one frame late and per model.** The render layer cannot publish while
  preparing a frame, so it hands events to the widget, whose next `update` publishes
  them. Two widgets drawing one model both receive all of them, and the second one to
  appear is sent the load result it missed.
- **No listener, no channel.** Without `.on_event(..)` the render layer allocates no
  channel at all, so events cannot pile up.
- **`MotionStarted` carries `(group, clip)` plus the clip's duration and loop flag.**
  `MotionFinishedPlaying` fires for clips that end, and playback then stops until the
  host asks again. A looping clip never finishes, which is why an idle loop needs no
  scheduler.
- **Pointer events belong to the widget, not the model.** Two widgets drawing one
  model keep separate hover, presses and drags: each reports input in its own space.
  Every event names its model and carries the position in three forms — widget pixels,
  clip space and model space (see `Sample`).
- **`pause` freezes the pose, not the input.** Nothing steps and the model shows its
  last frame, but the frame that input needs still happens, so hovering, clicking and
  scrolling keep working.
- **Dropping an asset or model frees its GPU state** at the end of the next rendered
  frame, not immediately.

### Interaction

Input reaches the host as messages the host defines. Every event names the `Live2dId`
of the model it happened on — a live2d widget is not a button, so its identity cannot
live in the message variant. One model has several hit areas and the same vocabulary
of clicks and drags serves every model, so the message is `(Live2dId, what happened)`
and the variant says only what happened.

**A widget reports one vocabulary, chosen once.** `live2d(..)` starts undecided and
`raw()` or `gestures(..)` picks the branch; each branch only has the hooks that fit
it, so the same physical action can never be handled twice:

```rust
live2d(&model).raw()                            // RawEvent:    raw facts, host classifies
live2d(&model).gestures(Gestures::default())    // GestureEvent: the component classifies
```

The choice decides the **event type**, so it comes before `on_event`: a raw widget
hands the closure `RawEvent<I>` and a classified one `GestureEvent<I>`. Both carry the
same model events and a different input arm — `Input(PointerEvent<..>)` versus
`Input(Gesture<..>)` — so a message type names the vocabulary it holds and a handler
cannot match the wrong one.

| Method | Available in | Reported | Payload |
|---|---|---|---|
| `on_event` | both | everything the component says | `Live2dEvent<I>` |
| `on_press` / `on_release` | `raw` | a button or finger went down / came up | `Pointing<I::HitArea>` |
| `on_drag` | `raw` | the pointer moved while held, once per frame | `Dragging<I::HitArea>` |
| `on_click` | `gestures` | a press that came back without moving, once the double-click window closes | `Tap<I::HitArea>` |
| `on_double_click` | `gestures` | a second click inside that window; the first is not reported | `Tap<I::HitArea>` |
| `on_long_press` | `gestures` | held still past the threshold, while still down | `Tap<I::HitArea>` |
| `on_drag_start` / `on_drag` / `on_drag_end` | `gestures` | a press that travelled | `Drag<I::HitArea>` |
| `on_scroll` | both | the wheel over the widget | `Scrolling` |
| `on_enter` / `on_exit` | both | the pointer reached / left a hit area | `Hovering<I::HitArea>` |
| `on_move` | both | the pointer moved over the widget | `Moving` |

```rust
live2d(&self.model)
    .gestures(Gestures::default())                 // classify clicks, drags, holds
    .cursor(|hover| match hover {                  // cursor shape, no message needed
        Hover { pressed: true, .. } => mouse::Interaction::Grabbing,
        Hover { area: Some(_), .. } => mouse::Interaction::Pointer,
        _ => mouse::Interaction::Idle,
    })
    .on_click(|tap| Message::Pet { model: tap.model, area: tap.area })
    .on_drag(|drag| Message::Turned { model: drag.model, by: drag.delta() })
    .on_scroll(|scroll| Message::Zoom { model: scroll.model, by: scroll.lines[1] })
    .on_event(Message::Live2d)                     // everything else, load failures included
```

Decisions worth knowing:

- **One vocabulary, not two.** A raw widget never reports a gesture and a classified
  widget never reports a raw pointer event, so nothing arrives twice. Raw input is the
  escape hatch: it carries the facts, including the instant each happened, and a host
  that wants a double click classifies it itself.
- **Raw events are facts, gestures are a judgement.** `Pressed`/`Released` are reported
  for every physical press, so on a raw widget a double click shows two of each;
  `Clicked`/`DoubleClicked` exist only with `gestures(..)`.
- **A click is never reported before it is known not to be a double click.** Click and
  double click are not a prefix code: at the first release there is nothing to tell them
  apart. So `Gestures::double_click = Some(window)` holds every click until the window
  closes — a second click inside it produces `DoubleClicked` and the first click is
  never reported, and a single click is reported when the window closes. One action, one
  message. The cost is that a single click is late by up to that window;
  `double_click: None` reports clicks immediately and turns double-click detection off.
- **A specific hook wins over `on_event`** for the events it covers, and a more specific
  hook that is *not* bound falls back to `on_event`. With neither, the event is dropped —
  that is why binding only `on_click` costs exactly one message per click, while binding
  `on_event` as well costs one per press, release and click.
- **`on_move` is the only continuous input.** Nothing is reported per frame unless it is
  bound.
- **With `gestures`, `on_drag` reports the classified drag**, which carries velocity;
  without it, the raw drag arrives, which does not. Either way it fires once per frame,
  never twice.
- **The hit area of a drag** is the one under the press, fixed for the whole drag —
  dragging off a head does not turn the drag into a drag of nothing.
- **A press is a promise.** Losing focus, or a touch the platform takes away, reports
  `Cancelled`, so a host's own state machine always has a way out.
- **`capture(true)`** stops a parent `scrollable` or draggable surface from also
  reacting while a press is held.
- **`Sample` carries the same point three ways**: widget pixels (what window dragging
  wants), clip space (zoom-independent) and model space (what hit areas and physics
  speak).

## Examples

Everything takes a model path, because none is shipped:

```bash
cargo run --release -p iced-live2d --example minimal  -- "path/to/model.model3.json" [second]
cargo run --release -p iced-live2d --features macros --example typed -- "path/to/model.model3.json"
cargo run --release -p iced-live2d --example interact -- "path/to/model.model3.json" [second]
cargo run --release -p iced-live2d --example policy   -- "path/to/model.model3.json"
cargo run --release -p live2d-render --example offscreen -- "path/to/model.model3.json" frame.png 900 30
cargo run --release -p live2d-render --example policy -- "path/to/model.model3.json" ParamMouthOpenY 0.8 auto
```

| Example | Shows |
|---|---|
| `minimal` | two models in one window through `Raw`, keyed by `Live2dId`; keys `R`/`E`/`P`/`1`/`2`, every component event printed |
| `typed` | a host-declared interface: typed keys for parameters, parts, motions and hit areas, and the load failure a model that lacks them produces |
| `interact` | every input mapped to a message: click, double click, long press, drag, drag end, scroll (zooms the pane), hover, and gaze driven by the pointer |
| `policy` (iced) | a mouth and breath layer over the first clip; `B`/`S`/`L` switch blend, stage and look blend live |
| `policy <Param> <v> <auto\|before\|after>` (render) | which parameters a write changes, and whether staging it before physics moves the physics outputs |
| `offscreen` | one frame into a PNG, no window |
| `dump` | what is in the `.moc3`: drawables, mask sets, canvas, motion groups |
| `inspect` | what is in the *files*: physics settings, motion curves, expression targets, and how the art sits in the canvas |
| `probe <x> <y>` | which drawables cover a model-space point, in paint order |
| `switch <Param> <v0> <v1>` | which drawables a parameter flips, and their metadata |
| `sweep` | what physics does along a horizontal sweep of the look-at target |
| `jolt` | how many pixels change per frame during a slow sweep, and where |
| `rig` | how cursor tracking is routed for a model, in both modes — no GPU needed |

`sweep` is the fastest way to chase a tracking bug: it prints the input parameter and
the physics outputs per frame and flags frame-to-frame jumps. Prefix it with
`SWEEP_DIRECT=<ParamId>` to write that parameter directly. `inspect` takes
`LIVE2D_PARAMS=<substring>[,<substring>…]` to dump matching parameters with their
ranges and current values, which is how you check a parameter exists before blaming the
renderer.

`offscreen` reads its whole state from the environment, so a capture is reproducible
from a command line:

| Variable | Meaning |
|---|---|
| `LIVE2D_TEXTURE_SCALE` | upload resolution multiplier, default `1.0` |
| `LIVE2D_MASK` | mask render-target edge in pixels, default `1024` |
| `LIVE2D_MOTION` / `LIVE2D_EXPR` | `group,clip` to play / expression index to apply before the capture |
| `LIVE2D_LOOK_AT` | `x,y` in `-1.0..=1.0`, applied as the cursor position |
| `LIVE2D_HIT` | `x,y` in `-1.0..=1.0`: print which hit area a click there would land in |
| `LIVE2D_FILL` | how much of the widget the model's bounds span (`1.0` touches the edges, default `0.925`) |
| `LIVE2D_PROFILE` | ask the widget for step/deform/upload cost and frame-interval statistics |

## Frame pacing

The widget drives its own frames at 60 Hz; **do not subscribe the app to
`window::frames`**. `Model::advance` takes whatever interval the host measured and
spends it in fixed 1/60 s steps, keeping the rest for the next call, so the pose is a
function of elapsed time alone rather than of how the host sliced it.

A host can hand the scene a zero-length interval, and desktop windows do: an iced
window delivers roughly 2–10% of its frames with an instant equal to the previous one.
Treating such a frame as real is what used to make parts flicker — `advance` reset every
parameter to its default and physics then returned before writing its outputs, so for
one frame the hair, skirt and accessories snapped to rest and back. Whatever the host
does, `advance` must not take a duplicate instant as progress.

`.profile(true)` reports the split, once per 60 frames:

```
live2d profile: 60 frames 1.00 steps, step 0.08 ms, deform 3.40 ms, upload 1.42 ms, delta 15.2..18.4 ms (mean 16.67)
```

`steps` above 1 means the host presents slower than 60 Hz and the model is catching up;
`step` is motion/expression/physics, `deform` the CPU mesh rebuild and `upload` the GPU
write. They are reported apart because on a 715-drawable rig the split says whether the
cost is simulation or buffers.

## How a model is framed

Framing comes from the **canvas** rectangle in the `.moc3` header — the box Cubism
Editor and the Viewer use — not from the bounding box of the artwork. A model whose art
leaves spare room in its canvas therefore looks smaller and off-centre in every host,
ours included: `Rice` has a 1.5:1 landscape canvas with the art filling 66% × 85% of it,
6% right and 6% below centre, so in a square window it occupies 61% × 53% of the
width and height where `Haru` occupies 40% × 83%. `LIVE2D_FILL` (or `Live2d::fill`)
decides how much of the widget those bounds span; the default is `0.925`, so a model
does not touch the edges.

## Memory

Textures dominate. The reference rig's five atlases are 592 MB uncompressed, and the
scene uploads at half resolution by default:

| | working set |
|---|---|
| naive (keeps decoded textures, full res) | 1 565 MB |
| assets dropped, full res | ~900 MB |
| assets dropped, `texture_scale: 0.5` (default) | **462 MB** |
| the above plus a mip chain (textures × 4/3) | ~511 MB |

Pass `AssetOptions { texture_scale: 1.0, .. }` for full quality (`SceneOptions` in the
render layer); mipmapped upload keeps thin line art intact when the model is drawn small,
which is why minification without a mip chain looks like dashed speckles. For reference,
the same model hosted in a Chromium webview running the official Cubism Web SDK measured
**3.2×** this app's private working set on the same machine.

## Pitfalls

- **A motion's fade comes from the model folder, not from the call.** A `model3.json`
  motion reference wins over the motion's own `Meta.FadeInTime` / `FadeOutTime`, and
  where neither declares one the fade is one second — the Cubism Viewer's default, not
  zero. `Live2dModel::set_fade` overrides both, and `Fade::zero()` cuts. Every replaced
  clip keeps fading out on its own, so several can overlap after a burst of switches;
  `set_fade_limit` caps how many.
- **Colour space:** the `format` handed to `Pipeline::new` is whatever the host surface
  uses. On a non-sRGB (`Bgra8Unorm`) target the model matches Cubism's gamma-space
  blending; on `*UnormSrgb` you must render into your own `Rgba8Unorm` target and do
  gamma→linear when compositing, or the model washes out.
- **The component's primitive must not implement `draw`.** It has to return `false` and
  do its work in `render`, because the mask pass has to be inserted between the passes
  iced controls.
- **`model3.json` is not always complete.** The reference rig declares zero motions and
  zero expressions while the files sit in `Animation/` and `Exp3/`.
  `AssetOptions::scan_sidecars` asks for a directory scan; what it finds lands in one
  synthesized group named `Scanned`, in alphabetical order of the files. It is off by
  default because a folder of files is not an animation set: this rig's `Animation/`
  holds one idle loop and seven one-shot "switch the hand" transitions, and only the
  host can tell them apart.
- **Look-at routing is resolved per model, not hardcoded.** `live2d-render/src/rig.rs`
  maps six slots (head yaw/pitch, body yaw/lean, eyes) onto whatever the rig exposes. A
  rig built for VTube Studio takes `AngleX`/`AngleY` and `PositionX`/`PositionY` because
  its `ParamAngle*` parameters are physics outputs; a classic rig takes the `Param*`
  names directly. The choice is decided from `physics3.json` and inspectable with
  `examples/rig`; `--track viewer` (the app) or `LIVE2D_TRACK=viewer` (examples) forces
  the Cubism Viewer's own binding set, which is what you want when comparing frames
  against the Viewer.
- **The component needs iced's `advanced` feature** — `Widget`, `Shell` and the layout
  types live behind it, and `shader::primitive::Renderer` is not publicly reachable, so
  the `Widget` impl bounds on `shader::Shader<Message, Self>` instead.

## Vendored mocari

`vendor/mocari` is the crates.io `0.4.1` release plus the four diffs in `patches/`,
committed so a fresh clone builds; the workspace dependency points straight at it, and
`patches/` is kept so the difference against upstream can be audited line by line.

Unpatched `0.4.1` cannot render this rig, and the fixes belong upstream rather than in
every consumer. Each patch file opens with what it corrects and how it was verified;
they are still being tested in more detail before being submitted upstream.

## Why iced comes from git

`mocari`'s wgpu backend needs `wgpu 30`, which is what iced **master** uses (`0.14.0` is
still on `wgpu 27`). Pinning master also pins the whole `wgpu` major, so the renderer
links straight into `Pipeline`. It is pinned by `rev`, and iced pins its own git
dependencies by `rev` too, so builds are reproducible. When 0.15 ships, switch to the
crates.io version — the glue should not need changes.

## License

MIT — see [LICENSE](LICENSE). The vendored `mocari` copy in `vendor/mocari` carries the
same MIT licence from its authors, and the model files used for the captures are not
part of the repository (see below).

## Model credits

Every capture in this README is a render of **DeepSeek-chan（大肥鱼）**, a freely
distributed model. The notice allows publishing content made with it — including
documentation — as long as the model is credited and the model files themselves are not
redistributed. **No model file is part of this repository.**

- 模型名称 / model: DeepSeek-chan（大肥鱼）
- 原画及拆分 / art and separation: 神宫凉子
- Live2D 建模 / rigging: 霓虹猫_NeonNeko
- 模型来源 / source: 神宫凉子免费配布模型

The sample characters used for comparisons are the official Cubism samples, used under
their own terms; [docs/test-models.md](docs/test-models.md) collects the provenance and
the licence text for each rig in the corpus.

## Documentation

- [docs/test-models.md](docs/test-models.md) — the model corpus: where each rig comes
  from, what it exercises, and where its terms of use live
