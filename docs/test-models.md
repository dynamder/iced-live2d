# Test corpus

This crate is driven by models, and every Live2D convention it has hit so far was
discovered by a rig that did something different. This is the list of models we
validate against, why each one is in the set, and where its terms of use live.

**No model asset is committed to this repository.** `.gitignore` excludes the
model directories and `*.moc3`; everything below lives under `.refs/` (ignored) and
is fetched by hand.

## Where the models come from

Every rig is a directory under `.refs/models/`, named after the model, holding the
`.model3.json` and everything it references:

```
.refs/models/
  Haru/  Hiyori/  Mao/  Mark/  Natori/  Ren/  Rice/  Wanko/
  大肥鱼/
```

The eight official characters ship with `Live2D/CubismWebSamples`, under
`Samples/Resources/<name>`; clone it anywhere and copy the character directories into
`.refs/models/`:

```bash
git clone --depth 1 https://github.com/Live2D/CubismWebSamples /tmp/cubism-samples
cp -r /tmp/cubism-samples/Samples/Resources/{Haru,Hiyori,Mao,Mark,Natori,Ren,Rice,Wanko} .refs/models/
```

Terms: the models are part of the Live2D Cubism Components, distributed under the
[Free Material License Agreement][fml]. Each character additionally has its own
"sample model terms" page — the closest thing the ecosystem has to an artist
statement, and the reason this list is limited to official samples:

- [Free Material License Agreement (en)][fml]
- [Sample model terms (en)][terms]

[Article 1.21][fml] distinguishes three licence types (free software, Live2D
original character, collaboration character), and the per-character terms page
adds character-specific conditions on top. Read them before publishing anything
rendered from these models; for *local testing* this project only needs them to
load, which the licence explicitly allows.

Community models published on Booth or shipped inside VTube Studio come with the
artist's own written terms rather than a license file. We do not add those to the
corpus automatically: someone has to read the statement and say yes. The rig used
during development (`DeepSeek 大肥鱼`) is such a case, and it was read: the notice
allows streaming, video, social posting and derivative works with credit, forbids
re-uploading the model files, and asks for a conversation before commercial projects
where the model is the subject. Documentation renders are therefore allowed, the
files stay out of git, and the credit lives in the README. Author: 原画及拆分
神宫凉子, Live2D 建模 霓虹猫_NeonNeko, distributed free by 神宫凉子.

## What each model exercises

The two commands that matter are:

```bash
cargo run -p live2d-render --example rig -- "<model>.model3.json"        # routing, no GPU
cargo run -p live2d-render --example offscreen -- "<model>.model3.json" out.png 400 30
```

| Model | Parameters | Convention resolved by `rig` | Physics | Pose | Motions | Expressions | Why it is here |
|---|---|---|---|---|---|---|---|
| Haru | 42 | `Param*` direct (convention) | yes | yes | 27 | 8 | small classic rig, lots of motions, expressions + pose |
| Hiyori | 70 | `Param*` direct | yes | yes | 10 | 0 | the canonical Cubism sample; no expressions |
| Mao | 132 | `Param*` direct | yes | yes | 8 | 8 | largest parameter set of the samples |
| Mark | 21 | `Param*` direct | yes | no | 6 | 0 | the smallest rig; good for spotting framing bugs |
| Natori | 96 | `Param*` direct | yes | yes | 8 | 11 | many expressions |
| Ren | 73 | `Param*` direct | yes | no | 3 | 5 | few motions, no pose |
| Rice | 96 | `Param*` direct, **no `ParamAngleY`** | yes | no | 4 | 0 | a rig that is missing a slot on purpose |
| Wanko | 25 | `PARAM_ANGLE_X` style | yes | no | 12 | 0 | old upper-case/underscore naming, no eye-ball parameters |
| 大肥鱼 | 509 | `AngleX` / `PositionX` (physics inputs) | yes (131 settings) | yes | 8 | 23 | the VTuber convention: look-at inputs feed physics outputs |

## What the formats disagree about

Six years of authoring show up in the files. Everything below was read off the
corpus with the loader this crate uses, and all nine parse.

| Model | `.moc3` | Optional sidecars declared | Motions (declared / files) | Fade declared in |
|---|---|---|---|---|
| Haru | 3.0.0 | physics, pose, user data, display info, expressions | 6 / 27 | `model3.json` 0.5 s |
| Hiyori | 4.0.0 | physics, pose, user data, display info | 10 / 10 | `model3.json` 0.5 s |
| Mao | 5.0.0 | physics, pose, display info, expressions | 8 / 8 | `Meta` 0.0 / 0.25 / 0.5 / 1.0 |
| Mark | 4.0.0 | physics, user data, display info | 6 / 6 | `model3.json` 0.5 s |
| Natori | 3.0.0 | physics, pose, display info, expressions | 8 / 8 | nowhere: 1.0 s |
| Ren | 5.3.0 | physics, display info, expressions | 3 / 3 | `Meta` 0.25 s on two |
| Rice | 4.0.0 | physics, display info | 4 / 4 | nowhere: 1.0 s |
| Wanko | 3.0.0 | physics, display info | 5 / 12 | nowhere: 1.0 s |
| 大肥鱼 | 5.0.0 | physics, display info | 0 / 8 (in `Animation/`) | nowhere: 1.0 s |

What that costs anyone reading these files:

- **A motion's fade is declared in one of two places.** `Motions.<group>[i]` in
  `model3.json` overrides `Meta.FadeInTime` / `Meta.FadeOutTime` in the
  `motion3.json`, and both fall back to 1.0 s. Three models declare it in the
  model file, two in the motion file, four nowhere. Read only one of the two and
  half the corpus fades for the wrong length.
- **Zero is a value, not "unset".** Mao declares 0.0 s on some clips — a hard cut
  the animator asked for — while leaving the key out on others. A parser that
  filters out falsy values turns that cut into one second.
- **`Motions` can be absent entirely**, not empty: 大肥鱼 has no key, and its
  eight clips sit in `Animation/`. Only a directory scan finds them, and a scanned
  clip has no `model3.json` entry, so it can only inherit the motion file's own
  fade or the default.
- **`exp3.json` and `pose3.json` carry no `Version`**, only `Type`; `physics3.json`
  and `cdi3.json` always do (`3`). Mao and Ren add `cdi3.json`'s
  `CombinedParameters`; 大肥鱼's expressions add VTube Studio's
  `IsModelCustomizationExpression`. Reading these files means ignoring what is not
  understood, never rejecting a document for having more in it.
- **Every one of the 86 motion files declares `Loop: true`**, including 大肥鱼's
  one-shot "switch the hand" transitions. A clip that ends on its own does not
  exist in this corpus; leaving one means `stop_motion` or starting another.
- **Model-target curves are data, not decoration.** Haru ships an `Opacity` curve
  in all 27 clips (constant 1.0 — dropping it costs nothing), while Mark ships an
  `EyeBlink` curve in all six that actually animates 0 to 1. `mocari` drops
  `Target: "Model"` curves today, so Mark's authored blinks are lost.

## What the corpus already caught

- **Wanko** uses `PARAM_ANGLE_X` / `PARAM_BODY_ANGLE_X` (upper case, underscores)
  and has no `PARAM_EYE_BALL_X`/`Y` at all. The resolver originally missed it
  entirely; `ModelFacts::find` now matches names case- and underscore-insensitively,
  and the eye slots are simply left unbound.
- **Rice** has no `ParamAngleY`. The resolver reports `head-pitch` as not driven
  instead of writing a parameter that does not exist.
- Every sample model resolves through the `convention` provider, i.e. the classic
  path had zero coverage before this corpus existed — the whole adapter had only
  ever been exercised by the one VTuber rig.

## Adding a model

1. Read its terms and write down where they are (a URL, or "the artist said yes in
   a DM" is not good enough for a repository).
2. Put it under `.refs/models/<name>/` so it stays out of git.
3. Run `examples/rig` and paste the resolved table into the table above.
4. Run `examples/offscreen` and eyeball the PNG.
5. If the resolver gets it wrong, add a fake-facts unit test in `rig.rs` before
   changing the rules.

[fml]: https://www.live2d.com/eula/live2d-free-material-license-agreement_en.html
[terms]: https://www.live2d.com/eula/live2d-sample-model-terms_en.html
