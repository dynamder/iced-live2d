# Change plan: motion indices, events, focus and freeze

Status: proposed. This is a working document, not user-facing API documentation.
Fold the durable decisions into `docs/gaps.md` / `README.md` and delete this file
before release.

Scope of this plan is the review round that started from the audit on 2026-11-20.
It is deliberately narrow:

- motion indices become newtypes now; the other index types wait for a later round;
- model events and interaction events become separate types;
- `on_event` is the interaction catch-all; model events have their own channel;
- pointer focus is maintained by the component;
- touch input is removed;
- the remaining agreed fixes and the CI baseline are included in later phases.

## Locked decisions

1. **Motion only.** Add `MotionGroupIndex` and `MotionClipIndex` in this round.
   `ParameterIndex`, `PartIndex`, `ExpressionIndex`, `HitAreaIndex` and
   `DrawableIndex` are a separate follow-up.
2. **`on_event` is interaction-only.** `on_event` receives every event of the
   current interaction vocabulary (`RawInput` or `GestureInput`). It does not
   receive `ModelEvent`.
3. **Model events have their own channel.** `on_model_event` is always available
   and receives `ModelEvent<I>`. It may be combined with `on_event` or with
   specific interaction hooks because the two categories do not overlap.
4. **Interaction hooks are exclusive with `on_event`.** Once `on_event` is
   called, no specific interaction hook is available. Once a specific interaction
   hook is called, `on_event` is not available. This is enforced by the type
   state, not at runtime.
5. **Focus is component-owned.** One `Model` shared by several widgets is one
   logical model; animation state is shared, interaction state is view-local,
   and one view at a time owns the pointer.
6. **Touch is not supported.** Remove the touch input paths instead of leaving a
   half-working implementation.
7. **`pause` becomes `freeze`.** Freeze stops time (no step, deform or upload),
   but input and hit-testing continue.
8. **mocari is not restructured.** The per-instance `ModelRuntime` clone is
   recorded as a known limitation; splitting static data from mutable state is a
   later mocari/vendor effort.

## Non-goals

- Changing `ModelRuntime` or any other mocari data structure.
- Migrating from a path dependency to `[patch.crates-io]`.
- Publishing to crates.io.
- MSRV verification.
- GPU or window integration tests in CI.
- Newtypes for parameter, part, expression, hit-area or drawable indices.
- Replacing `Profile` with `hotpath`.

## PR sequence

Each phase is independently buildable and testable. Do not combine the event
split with focus in one PR.

| PR | Content |
|---|---|
| 1 | Motion index newtypes, `motion_clips -> Option`, motion error variants |
| 2 | Model/interaction event split and pipeline event split |
| 3 | Handler type state, `on_model_event`, `on_event` semantics |
| 4 | Event queue coalescing, queue cap and `EventQueueOverflow` |
| 5 | Per-view pointer queue, model-level focus, touch removal |
| 6 | `freeze` rename, asset checks, asset issues, small agreed fixes |
| 7 | CI, vendor test job, documentation sync |
| 8 | Comment cleanup (optional, after the functional changes) |

---

## Phase 0: baseline

No code changes.

- Run the baseline commands from "Test commands" below.
- Record the known limitations in `docs/gaps.md`:
  - per-instance `ModelRuntime` clone (measured ~31 MB for the 715-drawable rig,
    ~0.9 MB for Hiyori) and the mocari split as the closing cost;
  - `Prepared` retaining decoded textures after an interface mismatch;
  - other index newtypes deferred.
- Confirm that `cargo fmt --all --check` is not a CI command: it formats the
  vendor path dependency and would invalidate the byte-exact patches.

## Phase 1: motion indices and errors

Goal: no public motion API uses a bare `usize` for a group or clip, and the
"how many groups" / "how many clips" fields cannot be confused.

### 1.1 New index types

New file `crates/live2d-render/src/index.rs`:

```rust
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MotionGroupIndex(usize);

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MotionClipIndex(usize);
```

- Add `new(usize)`, `get() -> usize`, `Display`.
- `new` stays public and is documented as a domain marker, not proof that the
  index belongs to a particular model. `Model::play_motion` still validates the
  range and reports an error.
- Export both from `live2d-render/src/lib.rs`.

### 1.2 `ModelInfo`

`crates/live2d-render/src/info.rs`:

```rust
pub fn motion_clips(&self, group: MotionGroupIndex) -> Option<usize>;
pub fn motion_at(&self, index: usize) -> Option<MotionGroupIndex>;
pub fn motion_clip(
    &self,
    group: MotionGroupIndex,
    clip: usize,
) -> Option<MotionClipIndex>;
```

- `motion_clips` returns `None` for an unknown group; it no longer invents `0`.
- Remove every "model order" claim. Document:
  > List order is an implementation detail. It is stable within one loaded asset,
  > but it is not the `model3.json` order. Use names for semantics and indices
  > only as identity within that asset.
- `RawKey::index` remains a vocabulary index, not a model index; document the
  distinction.

### 1.3 `Error`

`crates/live2d-render/src/error.rs`:

```rust
UnknownMotionGroup {
    group: MotionGroupIndex,
    groups: usize,
},
UnknownMotionClip {
    group: MotionGroupIndex,
    clip: MotionClipIndex,
    clips: usize,
},
EmptyMotionGroup {
    group: MotionGroupIndex,
},
```

- `groups` and `clips` are counts, not indices; they stay `usize`.
- `UnknownMotionGroup.groups` is the total number of groups in the model.
- `UnknownMotionClip.clips` is the number of clips in the existing group.
- `EmptyMotionGroup` covers `play_random_motion` on an existing empty group.

### 1.4 `Model`

`crates/live2d-render/src/model.rs`:

```rust
pub fn play_motion(&mut self, group: MotionGroupIndex, clip: MotionClipIndex);
pub fn play_random_motion(&mut self, group: MotionGroupIndex);
pub fn current_motion(&self) -> Option<(MotionGroupIndex, MotionClipIndex)>;
```

- `play_motion`: `motion_clips(group) == None` -> `UnknownMotionGroup`;
  `clip.get() >= clips` -> `UnknownMotionClip`.
- `play_random_motion`: `None` -> `UnknownMotionGroup`; `Some(0)` ->
  `EmptyMotionGroup`; otherwise pick uniformly.
- `Started` and `Ended` use the new index types.
- `take_started` / `take_ended` expose the new types.

### 1.5 iced layer

- `crates/iced-live2d/src/model.rs`: `play_motion` / `play_random_motion` take
  `MotionClipIndex` for the clip. `Command` keeps the typed key and the clip; it
  does not gain an `AssetId`.
- `crates/iced-live2d/src/pipeline.rs`:
  - `PrimitiveCommand::PlayMotion { group: usize, clip: MotionClipIndex }`,
    where `group` is still the vocabulary index;
  - `Mapping::motion(usize) -> Option<MotionGroupIndex>`;
  - `Mapping::motion_vocabulary(MotionGroupIndex) -> Option<usize>`;
  - call `Model::play_motion` with the mapped `MotionGroupIndex`.
- `crates/iced-live2d/src/event.rs`: `MotionStarted` / `MotionEnded` clip field
  becomes `MotionClipIndex` (Phase 2 will move these into `ModelEvent`).
- Examples use `MotionClipIndex::new(0)` or `ModelInfo::motion_clip`.

### 1.6 Tests

- `motion_clips` is `None` for an unknown group.
- `play_motion` on an unknown group reports the total group count.
- `play_motion` with an out-of-range clip reports the group's clip count.
- `play_random_motion` distinguishes unknown group and empty group.
- `current_motion` round-trips the newtypes.
- Existing examples compile after the signature changes.

Acceptance: no motion public API has `group: usize` or `clip: usize`; no error
field carries an ambiguous `count`.

---

## Phase 2: model and interaction events

### 2a. Event split

`crates/iced-live2d/src/event.rs`:

```rust
pub enum ModelEvent<I: Interface> {
    Loaded {
        model: Live2dId,
        info: Arc<ModelInfo>,
    },
    FailedToLoad {
        model: Live2dId,
        failure: LoadFailure,
    },
    Problem {
        model: Live2dId,
        problem: Problem,
    },
    MotionStarted {
        model: Live2dId,
        group: I::Motion,
        clip: MotionClipIndex,
        duration: f32,
        looping: bool,
    },
    MotionEnded {
        model: Live2dId,
        group: I::Motion,
        clip: MotionClipIndex,
        ended: MotionEnd,
    },
}
```

Interaction aliases:

```rust
pub type RawInput<I> = PointerEvent<<I as Interface>::HitArea>;
pub type GestureInput<I> = Gesture<<I as Interface>::HitArea>;
```

- Remove `Live2dEvent<I, In>`; model and interaction events are separate types.
- Remove the old `RawEvent` / `GestureEvent` aliases or rename them to the input
  aliases above.
- `ModelEvent` does not have an `In` parameter.

`crates/iced-live2d/src/pipeline.rs`:

```rust
pub(crate) enum PipelineEvent {
    Model(ModelEventKind),
    Pointer(PointerEventKind),
}
```

- Split `translate` into `translate_model` and `pointer_payload`.
- Model events broadcast to every view; pointer events stay view-local.
- Delete the `unreachable!` pointer arm in `widget.rs`.

### 2b. Handler type state

`crates/iced-live2d/src/widget.rs`:

```rust
pub struct NoInputHandler;
pub struct SpecificInput;
pub struct CatchAllInput;

pub struct Live2d<
    'a,
    Message,
    I: Interface,
    M = Unset,
    H = NoInputHandler,
> { ... }
```

Rules:

| State | Available |
|---|---|
| `Unset` + `NoInputHandler` | `on_model_event`, `raw`, `gestures` |
| `Mode` + `NoInputHandler` | `on_model_event`, `on_event`, any specific interaction hook |
| `Mode` + `SpecificInput` | `on_model_event`, other specific interaction hooks; no `on_event` |
| `Mode` + `CatchAllInput` | `on_model_event`; no specific interaction hook |

- `on_event` exists only for `M: InputMode, H = NoInputHandler`:
  ```rust
  pub fn on_event(
      self,
      f: impl Fn(M::Input<I::HitArea>) -> Message + 'a,
  ) -> Live2d<'a, Message, I, M, CatchAllInput>;
  ```
- Every specific interaction hook (`on_click`, `on_double_click`,
  `on_long_press`, `on_drag_start`, `on_drag`, `on_drag_end`, `on_press`,
  `on_release`, `on_scroll`, `on_enter`, `on_exit`, `on_move`) is available on
  `H = NoInputHandler` and `H = SpecificInput`; calling one returns
  `H = SpecificInput`.
- `on_model_event` is available on every `H`, including `CatchAllInput`,
  because the categories do not overlap.
- `Unset` has no `on_event`; call `.raw()` or `.gestures(..)` first.
- `raw()` / `gestures(..)` consume `Unset`, preserve the model handler and
  return `H = NoInputHandler`.
- Delete `cast`; no runtime handler reset.

### 2c. Dispatch

`Handlers` splits into:

```rust
struct Handlers<'a, Message, I, M> {
    model: Option<Handler<ModelEvent<I>, Message>>,
    input_any: Option<Handler<M::Input<I::HitArea>, Message>>,
    // specific interaction hooks...
}
```

- The pending queue stores an internal sum:
  ```rust
  enum PendingEvent<I, In> {
      Model(ModelEvent<I>),
      Input(In),
  }
  ```
- Model events go to `handlers.model`; interaction events go to `input_any` or a
  specific hook.
- The type state guarantees `input_any` and specific hooks are never both
  present.
- `on_event` receives every interaction event, including `Moved`; remove the
  early returns that currently bypass `on_event` for `Moved`.

### 2d. `Vocabulary::from_id` returns `Result`

- Add `VocabularyError` carrying at least the reported index.
- Trait signature:
  ```rust
  fn from_id(id: &KeyId) -> Result<Self, VocabularyError>;
  ```
- Derived implementation: an out-of-range index is `Err`, not `None`.
- `Raw`: always `Ok`; index validity is checked by `Mapping` / `accepts`.
- Hand-written implementation: index/`from_id` disagreement is `Err`.
- Conversion failure becomes `ModelEvent::Problem(Problem::Vocabulary { vocabulary, index })`.
- Name remains diagnostic; the index is authoritative. Document that `from_id`
  failure means an internal render/vocabulary mismatch, not a normal missing
  entry.

### 2e. Backpressure

`crates/iced-live2d/src/widget.rs`:

- Cap the pending queue (64 is a reasonable starting value).
- Coalesce `Moved` to the latest event per view.
- Coalesce `Dragged` to the latest event per pointer.
- Keep discrete events (`Pressed`, `Released`, `Clicked`, `Scrolled`, ...) in
  order.
- On overflow, drop the oldest event and emit:
  ```rust
  Problem::EventQueueOverflow { dropped: usize }
  ```
  as a model event. If no `on_model_event` is bound, the problem is dropped like
  any other model event.
- Keep one message per frame; the cap prevents unbounded growth.

### 2f. Tests and examples

- `trybuild` UI tests (new dev-dependency, `tests/ui`):
  - `on_event` then `on_click` fails;
  - `on_click` then `on_event` fails;
  - `on_event` on `Unset` fails;
  - `on_event` then `on_press` fails;
  - a specific hook on `CatchAllInput` fails;
  - `on_model_event` + specific hook compiles;
  - `on_model_event` + `on_event` compiles.
- Runtime tests: `on_event` receives `Moved`; model events only go to
  `on_model_event`; queue coalescing and overflow.
- Migrate `minimal`, `app`, `interact`, `typed`, iced `policy`.
- Update README snippets and event names.

Acceptance: no `Live2dEvent` sum type; `on_event` is interaction-only; `on_event`
and specific interaction hooks cannot coexist; `Moved` is delivered to
`on_event`; pending cannot grow without bound.

---

## Phase 3: focus and per-view pointer input

### 3.1 Per-view input queue

- Add `pointer: RefCell<Vec<PointerInput>>` to `Live2dState`.
- `queue_input` pushes to `state.pointer`, not to the model.
- `draw` drains `state.pointer` into `Live2dPrimitive.inputs.pointer`.
- Delete `Inner.pointer`, `Inner::push_pointer` and `Inner::drain_pointer`.
- `PointerInput` no longer needs a `ViewId`; the queue is already per-view.

### 3.2 Model-level focus

- Keep `ViewId` in the pipeline; the UI-side focus does not need it.
- `Inner` gains a weak reference to the owning view's `Presence`:
  ```rust
  focus: RefCell<Option<Weak<Presence>>>,
  ```
  A `Weak` can be stored on the UI side without keeping a dead view alive, and
  `queue_input` can upgrade it to check the owner. If the owner was dropped, the
  upgrade fails and the next press may claim focus.
- Replace `Live2dState.pressed: bool` with a pointer set:
  ```rust
  pressed: RefCell<BTreeSet<Pointer>>,
  ```
  `Pointer` needs `PartialOrd + Ord` for this (or use a small `Vec<Pointer>`).
  With touch removed this only contains mouse buttons, but the set prevents a
  second button release from clearing the first.
- `Hover { pressed }` becomes `!pressed.is_empty()`.

`queue_input` policy:

- `ButtonPressed` over the widget:
  - no focus -> claim focus for this view;
  - focus is this view -> enqueue;
  - focus is another view -> ignore.
- `ButtonReleased`:
  - only the focus view enqueues;
  - remove the button from `pressed`;
  - clear focus when the set becomes empty.
- `Moved` / `Dragged`: enqueue only for the focus view.
- `WheelScrolled`: ignore when another view owns focus; otherwise process in the
  view under the cursor.
- `Window::Unfocused`: enqueue `CancelAll`, clear `pressed`, clear focus.

`capture` continues to use `pressed`; only the focus view can set it.

### 3.3 Interaction cleanup on focus change

- Clear the old view's `Interaction.hover` / `down`.
- If the old view had a hover, emit one `Exited` with the last sample so the host
  does not keep a stale hover.
- The new view starts from an empty interaction state.
- Model events stay broadcast to all views.

### 3.4 Touch removal

- Delete the four `iced::Event::Touch` arms in `widget.rs`.
- Delete `use iced::touch`.
- Delete `Pointer::Touch`; `Pointer` keeps only `Mouse` for now.
- Delete `PointerInput::Cancelled` (only touch produced it); keep `CancelAll`
  and the public `PointerEvent::Cancelled`, which `Window::Unfocused` still
  produces for every held pointer.
- Remove touch-only comments and docs; update the `Cancelled` docs to say
  "window lost focus" rather than "touch taken away".
- Add "touch input (not supported yet)" to README's out-of-scope section and
  `docs/gaps.md`.
- Confirm `rg 'Touch|touch::'` only finds documentation or future notes.

### 3.5 Tests

- Two `Live2dState`s sharing one model:
  - pressing A queues only A's press;
  - pressing B while A owns focus is ignored;
  - releasing A clears focus;
  - B can claim focus on the next press.
- Multi-button: releasing the right button does not clear a held left button.
- A non-focus view receives no `Moved` / `Dragged`.
- No touch input path compiles or remains.

Acceptance: one model plus two widgets has exactly one interaction owner at a
time; pointer queues are per-view; touch is gone.

---

## Phase 4: freeze, asset checks and small fixes

### 4.1 `pause` -> `freeze`

- Rename `Live2dModel::pause` to `freeze`, `is_paused` to `is_frozen`.
- Rename `Inner.paused`, `FrameInputs.paused`, pipeline locals and widget checks.
- Semantics: no step, no deform, no upload; input, hit-testing and view reframing
  continue.
- Update `app` (key and status), `minimal`, `interact` and iced `policy`.
- Update README and `docs/gaps.md`; remove the "no pointer events" claim from
  `Live2dModel::freeze` docs and the app status line.

### 4.2 Asset check at the call site

`Live2dModel::play_motion`, `play_random_motion` and `play_expression` call
`Inner::accepts` before `push_command`:

```rust
if !self.inner.accepts(group.asset(), "motion", group.name()) {
    return;
}
```

- `Command` does not gain an `AssetId`.
- Add `"motion"` and `"expression"` to `Problem::ForeignKey`.
- Test that a `Raw` key from another asset is rejected and no command is queued.

### 4.3 Asset issues

- Collect `Prepared::issues()` before upload and `Asset::take_issues()` after
  upload in `build_model`.
- Store them as initial `Problem`s on the `ModelSlot` / `Ready`.
- Deliver them as `ModelEvent::Problem`, either on first registration or on the
  first reconcile.
- Make `scan_sidecars` return per-entry errors instead of `entries.flatten()`
  swallowing them.

### 4.4 Unmapped writes and commands

- In `pipeline::reconcile`, every `filter_map` drop for parameters/parts and
  every `None` from `mapping.motion` / `mapping.expression` emits
  `Problem::Vocabulary`.
- No silent no-ops for hand-written vocabularies or out-of-range Raw indices.

### 4.5 `mask_size`

- Fix the `MIN_MASK_SIZE` doc: it is a lower bound.
- Clamp the requested size to `device.limits().max_texture_dimension_2d`.
- Document the clamp and the lower bound.

### 4.6 `ModelBounds`

- Replace `ModelBounds::new` with a validated constructor:
  ```rust
  pub fn try_new(min: [f32; 2], max: [f32; 2]) -> Result<Self, InvalidBounds>;
  ```
- `from_canvas` / `from_drawables` keep their internal construction.
- `fit_matrix` no longer accepts negative extents.
- Update tests: reversed corners are rejected, not silently inverted.

### 4.7 Example names

- Rename `crates/iced-live2d/examples/policy.rs` to `policy-iced.rs`.
- Rename `crates/live2d-render/examples/policy.rs` to `policy-render.rs`.
- Update README commands and table.
- The two examples no longer share `target/debug/examples/policy`.

### 4.8 `Vocabulary::from_id` follow-up

Already covered in Phase 2d. If Phase 2 is merged without it, keep it as a
separate PR in this phase.

### 4.9 Other small cleanups

- Rename `RawKey`'s inherent `name` / `index` to `key_name` / `key_index`, or add
  a comment explaining the inherent/trait shadowing.
- Remove the stale `stop_motion` "no fade-out" entry from `docs/gaps.md`.
- Keep `Profile`; it is temporary and will be replaced by `hotpath` later.

---

## Phase 5: CI, vendor and documentation

### 5.1 CI

Add `.github/workflows/ci.yml` with:

```yaml
fmt:
  cargo fmt --check -p iced-live2d -p live2d-render -p iced-live2d-macros -p app

check:
  cargo check --workspace --all-targets --locked

test:
  cargo test --workspace --all-features --locked

clippy:
  cargo clippy --workspace --all-targets --all-features -- -D warnings

doc:
  RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps

vendor:
  cargo test --manifest-path vendor/mocari/Cargo.toml --locked
```

- Do not use `cargo fmt --all`; it would format the vendor path dependency and
  break byte-exact patch application.
- `--all-features` is required to run the macro tests.
- Give the vendor job its own target directory/cache.
- Do not add a GPU/offscreen job.
- MSRV stays out until a 1.93 toolchain is available and verified.

### 5.2 Vendor

- Keep `exclude = ["vendor"]`; do not restructure mocari.
- Keep the existing path dependency for now.
- Record the publish caveat:
  - a path dependency without a version cannot be published;
  - a versioned path dependency is stripped on publish, so consumers would get
    upstream 0.4.1 without the patches;
  - `[patch.crates-io]` is local-only and does not affect downstream;
  - publish requires upstreaming the patches or publishing a distinct fork
    version.
- Keep the vendor tests in a dedicated CI job; they are not part of
  `cargo test --workspace`.

### 5.3 Documentation sync

Update documents touched by this plan:

- `README.md`: event split, `on_model_event`, `freeze`, touch out-of-scope,
  motion index examples, per-instance memory wording, renamed policy examples.
- `docs/gaps.md`: freeze, touch, per-instance runtime memory, interface mismatch
  texture retention; remove stale `stop_motion` and pause entries.
- `docs/test-models.md`: correct the "113 clips" count to the actual 86, and
  record the `rig` index/name issue if not fixed.
- Correct known stale references while in these files:
  - `Live2dEvent::MotionFinishedPlaying` -> `ModelEvent::MotionEnded`;
  - "three examples" -> four iced examples;
  - "four diffs" -> five patches;
  - the 37 MB half-size figure: `0.5` is one quarter of the pixels, so the
    figure is closer to 148 MB; 37 MB corresponds to about `0.25`;
  - the `window::frames` contradiction in iced `policy`;
  - missing `offscreen` / `jolt` knobs.

Do not duplicate the gap list into README; keep `docs/gaps.md` as the source and
link it from README.

### 5.4 Comment cleanup

Rules:

- doc comments only for public API, one to three lines, usage and pitfalls;
- no design history, no "why we chose this" narrative;
- code comments only for genuinely non-obvious mechanics;
- move the durable rationale to `docs/design.md` or the commit message;
- clean the files touched by this plan first, then do a repository-wide sweep in
  a separate PR.

---

## Test commands

```bash
cargo fmt --check -p iced-live2d -p live2d-render -p iced-live2d-macros -p app
cargo check --workspace --all-targets --locked
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
cargo test --manifest-path vendor/mocari/Cargo.toml --locked
```

Manual checks outside CI:

```bash
cargo run --release -p live2d-render --example offscreen -- \
  .refs/models/Hiyori/Hiyori.model3.json /tmp/frame.png 256 10
cargo run --release -p live2d-render --example rig -- \
  .refs/models/Hiyori/Hiyori.model3.json
```

## Acceptance criteria

- No motion public API uses bare `group: usize` / `clip: usize`.
- `motion_clips` returns `Option`; motion errors distinguish unknown group,
  unknown clip and empty group.
- `Live2dEvent` is gone; model events and interaction events are separate types.
- `on_event` receives every interaction event, including `Moved`.
- `on_event` and specific interaction hooks are mutually exclusive at compile
  time.
- `on_model_event` remains available in every handler state.
- One `Model` shared by two widgets has one interaction owner at a time; both
  widgets receive model events.
- Touch input paths and `Pointer::Touch` are removed.
- `pause` no longer appears in the public API; `freeze` semantics are documented
  consistently and input keeps working.
- Foreign motion / expression keys are rejected at the call site without adding
  an `AssetId` to `Command`.
- Asset issues reach the host as model events.
- CI covers fmt, check, test (all features), clippy, doc and the vendor tests.

## Deferred and recorded

| Item | Decision |
|---|---|
| `ModelRuntime` static/state split | Record in `docs/gaps.md`; mocari/vendor work, not this round |
| Other index newtypes | Landed: parameter, part, expression and hit-area positions. Drawable positions never surface, so they have no type |
| `Prepared` decoded textures after interface mismatch | Record; rework deferred |
| `[patch.crates-io]` / publish story | Record; no change now |
| MSRV 1.93 verification | Deferred until a toolchain is available |
| GPU/window integration tests | Not in CI |
| `Profile` -> `hotpath` | Later |
| Repository-wide comment cleanup | Separate PR after the functional work |
