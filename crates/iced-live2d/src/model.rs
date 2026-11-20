//! The objects a host owns: an asset, and the models driven from it.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use live2d_render::{
    AssetId, AssetOptions, Fade, LookAtBlend, LookAtMode, ModelOptions, MotionClipIndex,
    ParameterPolicy, ParameterWrite,
};

use crate::event::Problem;
use crate::interaction::Pointer;
use crate::interface::{Interface, InterfaceSpec, Vocabulary};
use crate::pipeline::PrimitiveCommand;
use iced::Point;
use std::time::Instant;

/// How many commands may wait for the render layer before the oldest are dropped.
const MAX_QUEUED_COMMANDS: usize = 64;

/// Model identities. The render layer keys its per-model state on them.
static NEXT_MODEL: AtomicU64 = AtomicU64::new(1);

/// One model file, shared by every instance driven from it.
///
/// Nothing is read at construction: the first frame that draws one of its models
/// loads it and reports [`Live2dEvent::ModelLoaded`](crate::Live2dEvent::ModelLoaded)
/// or [`Live2dEvent::ModelFailedToLoad`](crate::Live2dEvent::ModelFailedToLoad).
/// The textures go away one frame after the last model using them is dropped.
pub struct Live2dAsset {
    id: AssetId,
    path: Arc<PathBuf>,
    options: AssetOptions,
    presence: Arc<Presence>,
}

impl Live2dAsset {
    /// An asset to load from `path`, the `.model3.json`; everything else is found
    /// from there.
    ///
    /// A missing path is not rejected here — it fails on the first frame, as a load
    /// failure. Start from [`AssetOptions::default`] and override what matters: the
    /// texture scale alone decides whether a rig costs 592 MB of VRAM or a fraction.
    pub fn new(path: impl Into<PathBuf>, options: AssetOptions) -> Self {
        Self {
            id: AssetId::unique(),
            path: Arc::new(path.into()),
            options,
            presence: Arc::new(Presence),
        }
    }

    /// The identity every raw key minted from this asset carries.
    pub fn id(&self) -> AssetId {
        self.id
    }

    /// The file this asset was created from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The budget this asset was created with.
    pub fn options(&self) -> AssetOptions {
        self.options
    }

    pub(crate) fn path_arc(&self) -> Arc<PathBuf> {
        Arc::clone(&self.path)
    }

    pub(crate) fn presence(&self) -> Arc<Presence> {
        Arc::clone(&self.presence)
    }
}

impl fmt::Debug for Live2dAsset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Live2dAsset")
            .field("id", &self.id)
            .field("path", &self.path)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

/// One instance of an asset: its own parameters, playback and physics.
///
/// `!Clone` and `!Send` on purpose — one owner, UI thread only. Commands take
/// `&self`; the options it was created with are read through [`Live2dModel::options`]
/// and the two that may change at runtime through
/// [`Live2dModel::parameter_policy_mut`] and [`Live2dModel::look_at_blend_mut`].
pub struct Live2dModel<I: Interface> {
    inner: Rc<Inner<I>>,
    options: ModelOptions,
}

pub(crate) struct Inner<I: Interface> {
    id: Live2dId,
    asset: AssetId,
    asset_options: AssetOptions,
    path: Arc<PathBuf>,
    spec: InterfaceSpec,
    parameters: RefCell<BTreeMap<I::Parameter, ParameterWrite>>,
    parts: RefCell<BTreeMap<I::Part, f32>>,
    commands: RefCell<VecDeque<Command<I>>>,
    look_at: Cell<Option<[f32; 2]>>,
    look_at_mode: Cell<LookAtMode>,
    fade: Cell<Option<Fade>>,
    fade_limit: Cell<Option<usize>>,
    paused: Cell<bool>,
    physics_enabled: Cell<bool>,
    pointer: RefCell<Vec<PointerInput>>,
    problems: RefCell<Vec<Problem>>,
    asset_presence: Arc<Presence>,
    reported: RefCell<BTreeSet<(&'static str, String)>>,
    presence: Arc<Presence>,
}

/// Proof that an asset or a model is still alive: the render layer holds a `Weak`
/// and frees the slot once it cannot upgrade it.
#[derive(Debug)]
pub(crate) struct Presence;

/// Identity of one model instance.
///
/// Stable for the model's lifetime, unlike a position in a `Vec`. Every event the
/// component sends carries it, so a host driving several models never has to guess
/// which one an event is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Live2dId(u64);

impl Live2dId {
    pub(crate) fn unique() -> Self {
        Self(NEXT_MODEL.fetch_add(1, Ordering::Relaxed))
    }
}

/// Something the host asked the model to do, queued until the render layer can.
pub(crate) enum Command<I: Interface> {
    PlayMotion {
        group: I::Motion,
        clip: MotionClipIndex,
    },
    PlayRandomMotion {
        group: I::Motion,
    },
    PlayExpression {
        expression: I::Expression,
    },
    StopMotion,
    ClearExpressions,
}

impl<I: Interface> Command<I> {
    /// Reduces the command to the plain data the render layer takes.
    pub(crate) fn primitive(&self) -> PrimitiveCommand {
        match self {
            Command::PlayMotion { group, clip } => PrimitiveCommand::PlayMotion {
                group: group.index(),
                clip: *clip,
            },
            Command::PlayRandomMotion { group } => PrimitiveCommand::PlayRandomMotion {
                group: group.index(),
            },
            Command::PlayExpression { expression } => PrimitiveCommand::PlayExpression {
                expression: expression.index(),
            },
            Command::StopMotion => PrimitiveCommand::StopMotion,
            Command::ClearExpressions => PrimitiveCommand::ClearExpressions,
        }
    }
}

/// One pointer input, queued by the widget and consumed by the render layer.
///
/// Positions are window pixels captured when the event happens, so a press that is
/// followed by a fast move is still hit-tested where it landed. Mouse movement is
/// not queued: it is ambient state the render layer reads each frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PointerInput {
    Pressed {
        pointer: Pointer,
        at: Point,
        when: Instant,
    },
    Released {
        pointer: Pointer,
        at: Point,
        when: Instant,
    },
    Moved {
        pointer: Pointer,
        at: Point,
    },
    Scrolled {
        lines: [f32; 2],
        at: Point,
        when: Instant,
    },
    Cancelled {
        pointer: Pointer,
    },
    CancelAll,
}

impl<I: Interface> Live2dModel<I> {
    /// An instance of `asset`, with the GPU budget and the write policy it is
    /// created with.
    ///
    /// The mask render target is per instance, so `mask_size` is the one budget an
    /// asset cannot share. There is no overload that invents the rest: pass
    /// [`ModelOptions::default`] when the defaults are what you want.
    pub fn new(asset: &Live2dAsset, options: ModelOptions) -> Self {
        Self {
            inner: Rc::new(Inner {
                id: Live2dId::unique(),
                asset: asset.id(),
                asset_options: asset.options(),
                path: asset.path_arc(),
                spec: InterfaceSpec::of::<I>(),
                parameters: RefCell::new(BTreeMap::new()),
                parts: RefCell::new(BTreeMap::new()),
                commands: RefCell::new(VecDeque::new()),
                look_at: Cell::new(None),
                look_at_mode: Cell::new(LookAtMode::default()),
                fade: Cell::new(None),
                fade_limit: Cell::new(None),
                paused: Cell::new(false),
                physics_enabled: Cell::new(true),
                pointer: RefCell::new(Vec::new()),
                problems: RefCell::new(Vec::new()),
                asset_presence: asset.presence(),
                reported: RefCell::new(BTreeSet::new()),
                presence: Arc::new(Presence),
            }),
            options,
        }
    }

    /// This instance's identity, which every event about it carries.
    pub fn id(&self) -> Live2dId {
        self.inner.id
    }

    /// The asset this instance was built from.
    pub fn asset(&self) -> AssetId {
        self.inner.asset
    }

    /// The file this instance was built from.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// The options this instance is running with, including the current write
    /// policy.
    pub fn options(&self) -> ModelOptions {
        self.options
    }

    /// How writes that do not name their own blend or stage are applied.
    pub fn parameter_policy(&self) -> ParameterPolicy {
        self.options.parameter_policy
    }

    /// Changes how unnamed writes are applied, from the next frame on.
    pub fn parameter_policy_mut(&mut self) -> &mut ParameterPolicy {
        &mut self.options.parameter_policy
    }

    /// How the built-in look-at combines with what the model plays.
    pub fn look_at_blend(&self) -> LookAtBlend {
        self.options.look_at_blend
    }

    /// Changes how the built-in look-at combines with the pose.
    pub fn look_at_blend_mut(&mut self) -> &mut LookAtBlend {
        &mut self.options.look_at_blend
    }

    /// The parameter writes of this instance.
    ///
    /// Values are **raw**, in the range the model declares — see
    /// [`ParameterRange`](crate::ParameterRange). Every step replays them after
    /// the motion and the expressions, and after look-at: a host write wins over
    /// all three. [`ParameterPolicy`] decides whether physics sees it and how it
    /// combines with what played.
    pub fn parameters(&self) -> Parameters<'_, I> {
        Parameters { model: &self.inner }
    }

    /// The part opacities of this instance, replayed every step.
    pub fn parts(&self) -> Parts<'_, I> {
        Parts { model: &self.inner }
    }

    /// Points head, body and eyes at `target`: `-1.0..=1.0` on both axes, `+y` up.
    ///
    /// The value persists, so a host without a cursor can call it once. A widget
    /// [`LookAtSource`](crate::LookAtSource) overrides it while the cursor is over
    /// the widget; this is where the model returns to afterwards.
    pub fn look_at(&self, target: [f32; 2]) {
        self.inner.look_at.set(Some(target));
    }

    /// Which parameters a LookAt target is written to.
    pub fn look_at_mode(&self) -> LookAtMode {
        self.inner.look_at_mode.get()
    }

    /// Switches where a LookAt target lands, from the next frame on. No reload
    /// needed; host values are untouched.
    pub fn set_look_at_mode(&self, mode: LookAtMode) {
        self.inner.look_at_mode.set(mode);
    }

    /// How long a motion fades when nothing in the model folder declares a time.
    ///
    /// `None` — the default — leaves the files alone and gives the gap
    /// [`Fade::default`], Cubism's one second. `Some(fade)` overrides even a value a
    /// motion file declares, which is how [`Fade::zero`] turns every switch into a
    /// cut.
    pub fn set_fade(&self, fade: Option<Fade>) {
        self.inner.fade.set(fade);
    }

    /// The fade this host set, if any.
    pub fn fade(&self) -> Option<Fade> {
        self.inner.fade.get()
    }

    /// Caps how many interrupted motions may still be fading at once.
    ///
    /// `None`, the default, lets every one of them run to its own end; a number drops
    /// the oldest, which is the one closest to invisible.
    pub fn set_fade_limit(&self, limit: Option<usize>) {
        self.inner.fade_limit.set(limit);
    }

    /// The cap on concurrent fade-outs, if any.
    pub fn fade_limit(&self) -> Option<usize> {
        self.inner.fade_limit.get()
    }

    /// Stops time — a **total freeze**: no motion, no physics, no pointer events,
    /// and the widget stops asking for frames. To stop only the motion, use
    /// [`Live2dModel::stop_motion`].
    pub fn pause(&self) {
        self.inner.paused.set(true);
    }

    /// Restarts time after [`Live2dModel::pause`].
    pub fn resume(&self) {
        self.inner.paused.set(false);
    }

    /// Whether time is stopped.
    pub fn is_paused(&self) -> bool {
        self.inner.paused.get()
    }

    /// Turns physics evaluation on or off, for a still frame or a diagnostic A/B.
    pub fn set_physics_enabled(&self, enabled: bool) {
        self.inner.physics_enabled.set(enabled);
    }

    /// Whether physics is evaluated.
    pub fn physics_enabled(&self) -> bool {
        self.inner.physics_enabled.get()
    }

    /// Plays `clip` of `group` from the next frame, stopping whatever was playing.
    ///
    /// An out-of-range pair is reported as a
    /// [`Problem::Renderer`](crate::Problem::Renderer), not ignored.
    pub fn play_motion(&self, group: I::Motion, clip: MotionClipIndex) {
        self.inner.push_command(Command::PlayMotion { group, clip });
    }

    /// Plays a random clip of `group` — the usual way to pick an idle clip when one
    /// finishes. The component has no idle policy of its own.
    pub fn play_random_motion(&self, group: I::Motion) {
        self.inner.push_command(Command::PlayRandomMotion { group });
    }

    /// Applies an expression on top of the motion. Applying the same one twice
    /// restarts it.
    pub fn play_expression(&self, expression: I::Expression) {
        self.inner
            .push_command(Command::PlayExpression { expression });
    }

    /// Fades out every expression currently applied.
    pub fn clear_expressions(&self) {
        self.inner.push_command(Command::ClearExpressions);
    }

    /// Stops the current motion, revealing the rest pose. Time keeps running, so
    /// physics settles and pointer events keep arriving.
    pub fn stop_motion(&self) {
        self.inner.push_command(Command::StopMotion);
    }
}

impl<I: Interface> fmt::Debug for Live2dModel<I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Live2dModel")
            .field("id", &self.inner.id)
            .field("asset", &self.inner.asset)
            .field("path", &self.inner.path)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl<I: Interface> Inner<I> {
    fn push_command(&self, command: Command<I>) {
        let dropped = {
            let mut commands = self.commands.borrow_mut();

            if commands.len() >= MAX_QUEUED_COMMANDS {
                commands.pop_front();
                commands.push_back(command);
                true
            } else {
                commands.push_back(command);
                false
            }
        };

        if dropped {
            self.report_dropped_command();
        }
    }

    fn report_dropped_command(&self) {
        let mut problems = self.problems.borrow_mut();

        for problem in problems.iter_mut() {
            if let Problem::CommandQueueOverflow { dropped } = problem {
                *dropped += 1;
                return;
            }
        }

        problems.push(Problem::CommandQueueOverflow { dropped: 1 });
    }

    /// Reports once per distinct cause, so a mistake repeated every frame is not
    /// buried in copies of itself.
    fn report_foreign(&self, vocabulary: &'static str, name: &str) {
        let mut reported = self.reported.borrow_mut();

        if !reported.insert((vocabulary, name.to_owned())) {
            return;
        }

        drop(reported);

        self.problems.borrow_mut().push(Problem::ForeignKey {
            vocabulary,
            name: name.to_owned(),
        });
    }

    fn accepts(&self, key: Option<AssetId>, vocabulary: &'static str, name: &str) -> bool {
        if key.is_some_and(|asset| asset != self.asset) {
            self.report_foreign(vocabulary, name);
            return false;
        }

        true
    }
}

/// The parameter writes of one model, keyed by the interface's vocabulary.
///
/// A key minted from another asset is rejected and reported once, instead of
/// landing on the same index of this model.
pub struct Parameters<'a, I: Interface> {
    model: &'a Inner<I>,
}

impl<I: Interface> Parameters<'_, I> {
    /// Overwrites what the motion, expressions and look-at produced, returning the
    /// write it replaced.
    ///
    /// The blend and stage follow the model's [`ParameterPolicy`]; use
    /// [`Parameters::apply`] to name either. `None` means there was no previous
    /// write — or that the key belongs to another asset and was rejected.
    pub fn insert(&self, key: I::Parameter, value: f32) -> Option<ParameterWrite> {
        self.write(key, ParameterWrite::new(value))
    }

    /// Adds `delta` to what the model produced, returning the write it replaced.
    ///
    /// The stage follows the policy, so a physics input is seen by the simulation.
    pub fn add(&self, key: I::Parameter, delta: f32) -> Option<ParameterWrite> {
        self.write(key, ParameterWrite::add(delta))
    }

    /// Multiplies what the model produced by `factor`, returning the write it
    /// replaced.
    pub fn multiply(&self, key: I::Parameter, factor: f32) -> Option<ParameterWrite> {
        self.write(key, ParameterWrite::multiply(factor))
    }

    /// Writes with an explicit blend and/or stage, returning the write it replaced.
    pub fn apply(&self, key: I::Parameter, write: ParameterWrite) -> Option<ParameterWrite> {
        self.write(key, write)
    }

    fn write(&self, key: I::Parameter, write: ParameterWrite) -> Option<ParameterWrite> {
        if !self.model.accepts(key.asset(), "parameter", key.name()) {
            return None;
        }

        self.model.parameters.borrow_mut().insert(key, write)
    }

    /// Removes the write for one key, leaving the model to play alone.
    pub fn remove(&self, key: &I::Parameter) -> Option<ParameterWrite> {
        self.model.parameters.borrow_mut().remove(key)
    }

    /// The write set for one key, if any — not the value the model shows.
    pub fn get(&self, key: &I::Parameter) -> Option<ParameterWrite> {
        self.model.parameters.borrow().get(key).copied()
    }

    /// Drops every write.
    pub fn clear(&self) {
        self.model.parameters.borrow_mut().clear();
    }

    /// How many writes are set.
    pub fn len(&self) -> usize {
        self.model.parameters.borrow().len()
    }

    /// Whether no writes are set.
    pub fn is_empty(&self) -> bool {
        self.model.parameters.borrow().is_empty()
    }
}

/// The part opacities of one model, keyed by the interface's vocabulary. Clamped to
/// `0.0..=1.0`; pose fading applies on top.
pub struct Parts<'a, I: Interface> {
    model: &'a Inner<I>,
}

impl<I: Interface> Parts<'_, I> {
    /// Sets an opacity, returning the value it replaced.
    pub fn insert(&self, key: I::Part, opacity: f32) -> Option<f32> {
        if !self.model.accepts(key.asset(), "part", key.name()) {
            return None;
        }

        self.model.parts.borrow_mut().insert(key, opacity)
    }

    /// Removes an override, restoring the part's own opacity.
    pub fn remove(&self, key: &I::Part) -> Option<f32> {
        self.model.parts.borrow_mut().remove(key)
    }

    /// The opacity set for one key, if any.
    pub fn get(&self, key: &I::Part) -> Option<f32> {
        self.model.parts.borrow().get(key).copied()
    }

    /// Drops every override.
    pub fn clear(&self) {
        self.model.parts.borrow_mut().clear();
    }

    /// How many overrides are set.
    pub fn len(&self) -> usize {
        self.model.parts.borrow().len()
    }

    /// Whether no overrides are set.
    pub fn is_empty(&self) -> bool {
        self.model.parts.borrow().is_empty()
    }
}

impl<I: Interface> Inner<I> {
    pub(crate) fn id(&self) -> Live2dId {
        self.id
    }

    pub(crate) fn asset(&self) -> AssetId {
        self.asset
    }

    pub(crate) fn asset_options(&self) -> AssetOptions {
        self.asset_options
    }

    pub(crate) fn path_arc(&self) -> Arc<PathBuf> {
        Arc::clone(&self.path)
    }

    pub(crate) fn spec(&self) -> InterfaceSpec {
        self.spec
    }

    pub(crate) fn presence(&self) -> Arc<Presence> {
        Arc::clone(&self.presence)
    }

    pub(crate) fn asset_presence(&self) -> Arc<Presence> {
        Arc::clone(&self.asset_presence)
    }

    pub(crate) fn target(&self) -> Option<[f32; 2]> {
        self.look_at.get()
    }

    pub(crate) fn paused(&self) -> bool {
        self.paused.get()
    }

    pub(crate) fn look_at_mode(&self) -> LookAtMode {
        self.look_at_mode.get()
    }

    pub(crate) fn fade(&self) -> Option<Fade> {
        self.fade.get()
    }

    pub(crate) fn fade_limit(&self) -> Option<usize> {
        self.fade_limit.get()
    }

    pub(crate) fn physics_enabled(&self) -> bool {
        self.physics_enabled.get()
    }

    pub(crate) fn parameter_writes(&self) -> Vec<(usize, ParameterWrite)> {
        self.parameters
            .borrow()
            .iter()
            .map(|(key, write)| (key.index(), *write))
            .collect()
    }

    pub(crate) fn part_indices(&self) -> Vec<(usize, f32)> {
        self.parts
            .borrow()
            .iter()
            .map(|(key, value)| (key.index(), *value))
            .collect()
    }

    pub(crate) fn drain_commands(&self) -> Vec<Command<I>> {
        self.commands.borrow_mut().drain(..).collect()
    }

    pub(crate) fn drain_pointer(&self) -> Vec<PointerInput> {
        std::mem::take(&mut *self.pointer.borrow_mut())
    }

    pub(crate) fn push_pointer(&self, input: PointerInput) {
        self.pointer.borrow_mut().push(input);
    }

    pub(crate) fn take_problems(&self) -> Vec<Problem> {
        self.problems.borrow_mut().drain(..).collect()
    }
}

impl<I: Interface> Live2dModel<I> {
    pub(crate) fn inner(&self) -> &Inner<I> {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::{Raw, RawMotion, RawParameter};
    use live2d_render::{ParameterBlend, ParameterStage};

    fn asset() -> Live2dAsset {
        Live2dAsset::new("model.model3.json", AssetOptions::default())
    }

    #[test]
    fn the_constructor_touches_no_files() {
        let asset = Live2dAsset::new("nowhere/gone.model3.json", AssetOptions::default());

        assert_eq!(asset.path(), Path::new("nowhere/gone.model3.json"));
    }

    #[test]
    fn a_model_inherits_the_asset_identity_and_budget() {
        let asset = Live2dAsset::new(
            "model.model3.json",
            AssetOptions {
                texture_scale: 0.25,
                scan_sidecars: false,
            },
        );
        let model = Live2dModel::<Raw>::new(&asset, ModelOptions::default());

        assert_eq!(model.asset(), asset.id());
        assert_eq!(model.path(), asset.path());
    }

    #[test]
    fn commands_keep_their_order_and_are_drained_once() {
        let model = Live2dModel::<Raw>::new(&asset(), ModelOptions::default());
        let group = RawMotion::from_id(&crate::interface::KeyId {
            asset: model.asset(),
            index: 0,
            name: Arc::from("Idle"),
        })
        .expect("a raw key");

        model.play_motion(group.clone(), MotionClipIndex::new(1));
        model.stop_motion();

        assert_eq!(model.inner.drain_commands().len(), 2);
        assert!(model.inner.drain_commands().is_empty());
    }

    #[test]
    fn the_queue_drops_the_oldest_command_and_reports_it_once() {
        let model = Live2dModel::<Raw>::new(&asset(), ModelOptions::default());
        let group = RawMotion::from_id(&crate::interface::KeyId {
            asset: model.asset(),
            index: 0,
            name: Arc::from("Idle"),
        })
        .expect("a raw key");

        for clip in 0..(MAX_QUEUED_COMMANDS + 6) {
            model.play_motion(group.clone(), MotionClipIndex::new(clip));
        }

        assert_eq!(model.inner.drain_commands().len(), MAX_QUEUED_COMMANDS);

        let problems = model.inner.take_problems();

        assert_eq!(problems.len(), 1);
        assert!(matches!(
            problems.first(),
            Some(Problem::CommandQueueOverflow { dropped: 6 })
        ));
    }

    #[test]
    fn a_key_from_another_asset_is_rejected_and_reported_once() {
        let own = asset();
        let model = Live2dModel::<Raw>::new(&own, ModelOptions::default());
        let foreign = RawParameter::from_id(&crate::interface::KeyId {
            asset: AssetId::unique(),
            index: 0,
            name: Arc::from("ParamAngleX"),
        })
        .expect("a raw key");

        assert_eq!(model.parameters().insert(foreign.clone(), 1.0), None);
        assert_eq!(model.parameters().insert(foreign, 2.0), None);
        assert!(model.parameters().is_empty());

        let problems = model.inner.take_problems();

        assert_eq!(problems.len(), 1);
        assert!(matches!(
            problems.first(),
            Some(Problem::ForeignKey {
                vocabulary: "parameter",
                ..
            })
        ));
    }

    #[test]
    fn a_key_from_the_same_asset_is_accepted() {
        let asset = Live2dAsset::new("model.model3.json", AssetOptions::default());
        let model = Live2dModel::<Raw>::new(&asset, ModelOptions::default());
        let own = RawParameter::from_id(&crate::interface::KeyId {
            asset: model.asset(),
            index: 0,
            name: Arc::from("ParamAngleX"),
        })
        .expect("a raw key");

        assert_eq!(model.parameters().insert(own.clone(), 0.5), None);
        assert_eq!(model.parameters().get(&own), Some(ParameterWrite::new(0.5)));
        assert_eq!(
            model.parameters().insert(own, 0.75),
            Some(ParameterWrite::new(0.5))
        );

        let problems = model.inner.take_problems();
        assert!(problems.is_empty());
    }

    #[test]
    fn a_key_holds_one_write_and_later_writes_replace_it() {
        let asset = Live2dAsset::new("model.model3.json", AssetOptions::default());
        let model = Live2dModel::<Raw>::new(&asset, ModelOptions::default());
        let own = RawParameter::from_id(&crate::interface::KeyId {
            asset: model.asset(),
            index: 0,
            name: Arc::from("ParamAngleX"),
        })
        .expect("a raw key");

        assert_eq!(model.parameters().insert(own.clone(), 0.5), None);
        assert_eq!(
            model.parameters().add(own.clone(), 0.25),
            Some(ParameterWrite::new(0.5))
        );
        assert_eq!(
            model.parameters().get(&own),
            Some(ParameterWrite::add(0.25))
        );
        assert_eq!(
            model.parameters().multiply(own.clone(), 2.0),
            Some(ParameterWrite::add(0.25))
        );
        assert_eq!(
            model.parameters().apply(
                own.clone(),
                ParameterWrite::new(7.0).at(ParameterStage::AfterPhysics)
            ),
            Some(ParameterWrite::multiply(2.0))
        );
        assert_eq!(
            model.parameters().remove(&own),
            Some(ParameterWrite::new(7.0).at(ParameterStage::AfterPhysics))
        );
        assert!(model.parameters().is_empty());
    }

    #[test]
    fn the_policy_changes_without_rebuilding() {
        let mut model = Live2dModel::<Raw>::new(&asset(), ModelOptions::default());

        assert_eq!(model.parameter_policy(), ParameterPolicy::default());
        assert_eq!(model.look_at_blend(), LookAtBlend::Overwrite);

        model.parameter_policy_mut().blend = ParameterBlend::Add;
        *model.look_at_blend_mut() = LookAtBlend::Add;

        assert_eq!(model.parameter_policy().blend, ParameterBlend::Add);
        assert_eq!(model.look_at_blend(), LookAtBlend::Add);
    }

    #[test]
    fn pause_and_physics_are_readable_back() {
        let model = Live2dModel::<Raw>::new(&asset(), ModelOptions::default());

        model.pause();
        model.set_physics_enabled(false);

        assert!(model.is_paused());
        assert!(!model.physics_enabled());

        model.resume();
        model.set_physics_enabled(true);

        assert!(!model.is_paused());
        assert!(model.physics_enabled());
    }

    #[test]
    fn the_last_look_at_value_wins() {
        let model = Live2dModel::<Raw>::new(&asset(), ModelOptions::default());

        model.look_at([0.2, 0.3]);
        model.look_at([-0.5, 0.75]);

        assert_eq!(model.inner.target(), Some([-0.5, 0.75]));
    }

    #[test]
    fn an_untouched_model_has_no_target() {
        assert_eq!(
            Live2dModel::<Raw>::new(&asset(), ModelOptions::default())
                .inner
                .target(),
            None
        );
    }

    #[test]
    fn models_get_distinct_ids_and_presences() {
        let asset = asset();
        let first = Live2dModel::<Raw>::new(&asset, ModelOptions::default());
        let second = Live2dModel::<Raw>::new(&asset, ModelOptions::default());

        assert_ne!(first.inner.id(), second.inner.id());
        assert_ne!(
            Arc::as_ptr(&first.inner.presence()),
            Arc::as_ptr(&second.inner.presence())
        );
    }
}
