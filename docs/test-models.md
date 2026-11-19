# Test corpus

This crate is driven by models, and every Live2D convention it has hit so far was
discovered by a rig that did something different. This is the list of models we
validate against, why each one is in the set, and where its terms of use live.

**No model asset is committed to this repository.** `.gitignore` excludes the
model directories and `*.moc3`; everything below lives under `.refs/` (ignored) and
is fetched by hand.

## Where the models come from

`Live2D/CubismWebSamples` ships the official sample characters:

```bash
git clone --depth 1 https://github.com/Live2D/CubismWebSamples .refs/cubism-web-samples
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
| DeepSeek 大肥鱼 | 509 | `AngleX` / `PositionX` (physics inputs) | yes (131 settings) | yes | 8 | 23 | the VTuber convention: look-at inputs feed physics outputs |

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
