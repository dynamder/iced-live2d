//! The objects a host owns: an asset, and the models driven from it.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use live2d_render::{AssetId, AssetOptions, LookAtMode, ModelOptions};

use crate::event::Problem;
use crate::interface::{Interface, InterfaceSpec, Vocabulary};
use crate::pipeline::PrimitiveCommand;

/// How many commands may wait for the render layer before the oldest are dropped.
const MAX_QUEUED_COMMANDS: usize = 64;

/// Source of model identities. The render layer keys its per-model state on them,
/// so they must not collide within a process.
static NEXT_MODEL: AtomicU64 = AtomicU64::new(1);

/// One model file, shared by every instance driven from it.
///
/// Build it once, keep it in the host's state, and hand it to
/// [`Live2dModel::new`]. Nothing is read at construction: the first frame that
/// draws one of its models loads it, and the outcome arrives as
/// [`Live2dEvent::ModelLoaded`](crate::Live2dEvent::ModelLoaded) or
/// [`Live2dEvent::ModelFailedToLoad`](crate::Live2dEvent::ModelFailedToLoad).
///
/// Several models of one asset share the textures and the parsed data; dropping
/// the asset releases them once the last model that used it is gone, at the end of
/// the next rendered frame.
pub struct Live2dAsset {
    id: AssetId,
    path: Arc<PathBuf>,
    options: AssetOptions,
    presence: Arc<Presence>,
}

impl Live2dAsset {
    /// An asset to load from `path` with [`AssetOptions::default`].
    ///
    /// `path` is the `.model3.json`; everything else is found from there. A path
    /// that does not exist is not rejected here — it fails on the first frame and
    /// is reported as a load failure.
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self::with_options(path, AssetOptions::default())
    }

    /// An asset with an explicit texture budget.
    pub fn with_options(path: impl Into<PathBuf>, options: AssetOptions) -> Self {
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

/// One instance of an asset, owned by the host.
///
/// An instance keeps its own parameter values, playback position and physics
/// state, so several of them can show the same asset in different poses. It is
/// `!Clone` and `!Send` on purpose: one owner, UI thread only.
///
/// Every method takes `&self`; the instance is driven through interior mutability
/// so a host can hold it in immutable state and still command it.
pub struct Live2dModel<I: Interface> {
    inner: Rc<Inner<I>>,
}

pub(crate) struct Inner<I: Interface> {
    id: ModelId,
    asset: AssetId,
    asset_options: AssetOptions,
    path: Arc<PathBuf>,
    options: ModelOptions,
    spec: InterfaceSpec,
    parameters: RefCell<BTreeMap<I::Parameter, f32>>,
    parts: RefCell<BTreeMap<I::Part, f32>>,
    commands: RefCell<VecDeque<Command<I>>>,
    look_at: Cell<Option<[f32; 2]>>,
    look_at_mode: Cell<LookAtMode>,
    paused: Cell<bool>,
    physics_enabled: Cell<bool>,
    pointer: RefCell<Vec<PointerInput>>,
    problems: RefCell<Vec<Problem>>,
    asset_presence: Arc<Presence>,
    reported: RefCell<BTreeSet<(&'static str, String)>>,
    presence: Arc<Presence>,
}

/// Proof that an asset or a model is still alive.
///
/// The render layer holds a `Weak` to one of these per slot and frees the slot
/// once it can no longer be upgraded.
#[derive(Debug)]
pub(crate) struct Presence;

/// Identity of one model instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ModelId(u64);

impl ModelId {
    fn unique() -> Self {
        Self(NEXT_MODEL.fetch_add(1, Ordering::Relaxed))
    }
}

/// Something the host asked the model to do, queued until the render layer can.
pub(crate) enum Command<I: Interface> {
    PlayMotion { group: I::Motion, clip: usize },
    PlayRandomMotion { group: I::Motion },
    PlayExpression { expression: I::Expression },
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PointerInput {
    Pressed,
    Released,
    Scrolled(f32),
}

impl<I: Interface> Live2dModel<I> {
    /// An instance of `asset` with [`ModelOptions::default`].
    pub fn new(asset: &Live2dAsset) -> Self {
        Self::with_options(asset, ModelOptions::default())
    }

    /// An instance with an explicit GPU budget.
    pub fn with_options(asset: &Live2dAsset, options: ModelOptions) -> Self {
        Self {
            inner: Rc::new(Inner {
                id: ModelId::unique(),
                asset: asset.id(),
                asset_options: asset.options(),
                path: asset.path_arc(),
                options,
                spec: InterfaceSpec::of::<I>(),
                parameters: RefCell::new(BTreeMap::new()),
                parts: RefCell::new(BTreeMap::new()),
                commands: RefCell::new(VecDeque::new()),
                look_at: Cell::new(None),
                look_at_mode: Cell::new(LookAtMode::default()),
                paused: Cell::new(false),
                physics_enabled: Cell::new(true),
                pointer: RefCell::new(Vec::new()),
                problems: RefCell::new(Vec::new()),
                asset_presence: asset.presence(),
                reported: RefCell::new(BTreeSet::new()),
                presence: Arc::new(Presence),
            }),
        }
    }

    /// The asset this instance was built from.
    pub fn asset(&self) -> AssetId {
        self.inner.asset
    }

    /// The file this instance was built from.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// The budget this instance was built with.
    pub fn options(&self) -> ModelOptions {
        self.inner.options
    }

    /// The parameter overrides of this instance.
    ///
    /// Values are **raw**, in the range the model declares — see
    /// [`ParameterRange`](crate::ParameterRange) and
    /// [`ModelInfo`](crate::ModelInfo). They are replayed every step, after the
    /// motion and before physics, so a host value wins over whatever the motion
    /// drives.
    pub fn parameters(&self) -> Parameters<'_, I> {
        Parameters { model: &self.inner }
    }

    /// The part opacities of this instance, replayed every step.
    pub fn parts(&self) -> Parts<'_, I> {
        Parts { model: &self.inner }
    }

    /// Points head, body and eyes at `target`, given in `-1.0..=1.0` on both axes
    /// (`-1` = left/top, `+y` = up).
    ///
    /// Which parameters that means is resolved per model and per
    /// [`LookAtMode`]. The value persists until it is set again, so a host without
    /// a cursor can call this once.
    ///
    /// While the widget has a [`LookAtSource`](crate::LookAtSource) the source
    /// wins when it has an answer (the cursor is over the widget); **this value is
    /// what the model returns to when the cursor leaves**, and what drives the
    /// model when there is no source at all.
    pub fn look_at(&self, target: [f32; 2]) {
        self.inner.look_at.set(Some(target));
    }

    /// Which parameters a LookAt target is written to.
    pub fn look_at_mode(&self) -> LookAtMode {
        self.inner.look_at_mode.get()
    }

    /// Switches where a LookAt target lands, from the next frame on.
    ///
    /// Both tables are resolved at load time, so switching never needs a reload
    /// and never touches the host's own values: the next step rewrites every
    /// parameter from scratch.
    pub fn set_look_at_mode(&self, mode: LookAtMode) {
        self.inner.look_at_mode.set(mode);
    }

    /// Stops time.
    ///
    /// This is a **total freeze**: no motion, no physics, no expression fade, and
    /// no pointer events — the widget stops asking for frames, so nothing would
    /// even be drawn. To stop a motion but keep interacting, use
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

    /// Plays `clip` of `group`, stopping whatever was playing.
    ///
    /// The request is queued and takes effect on the next frame. An out-of-range
    /// pair is reported as a [`Problem::Renderer`](crate::Problem::Renderer), not
    /// ignored.
    pub fn play_motion(&self, group: I::Motion, clip: usize) {
        self.inner
            .push_command(Command::PlayMotion { group, clip });
    }

    /// Plays a random clip of `group`.
    ///
    /// Which clip is random is decided by the render layer, because only it knows
    /// how many the group holds. This is the usual way for a host to pick an idle
    /// clip when one finishes — the component has no idle policy of its own.
    pub fn play_random_motion(&self, group: I::Motion) {
        self.inner.push_command(Command::PlayRandomMotion { group });
    }

    /// Applies an expression on top of the motion.
    ///
    /// Fading between expressions is the render layer's business; applying the
    /// same expression twice restarts it.
    pub fn play_expression(&self, expression: I::Expression) {
        self.inner
            .push_command(Command::PlayExpression { expression });
    }

    /// Fades out every expression currently applied.
    pub fn clear_expressions(&self) {
        self.inner.push_command(Command::ClearExpressions);
    }

    /// Stops the current motion, revealing the rest pose.
    ///
    /// Unlike [`Live2dModel::pause`] this keeps time running, so physics settles,
    /// expressions keep fading and pointer events keep arriving.
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
            .field("options", &self.inner.options)
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

    /// Records a problem once per distinct cause, so a host that repeats a mistake
    /// every frame is not buried in copies of it.
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

/// The parameter overrides of one model, keyed by the interface's vocabulary.
///
/// Insertion is checked: a key minted from another asset is rejected and reported
/// once, instead of landing on the same index of this model.
pub struct Parameters<'a, I: Interface> {
    model: &'a Inner<I>,
}

impl<I: Interface> Parameters<'_, I> {
    /// Sets an override, returning the value it replaced.
    ///
    /// `None` means the key was rejected (it belongs to another asset) or that
    /// there was no previous value.
    pub fn insert(&self, key: I::Parameter, value: f32) -> Option<f32> {
        if !self.model.accepts(key.asset(), "parameter", key.name()) {
            return None;
        }

        self.model.parameters.borrow_mut().insert(key, value)
    }

    /// Removes an override.
    pub fn remove(&self, key: &I::Parameter) -> Option<f32> {
        self.model.parameters.borrow_mut().remove(key)
    }

    /// The override set for one key, if any.
    pub fn get(&self, key: &I::Parameter) -> Option<f32> {
        self.model.parameters.borrow().get(key).copied()
    }

    /// Drops every override.
    pub fn clear(&self) {
        self.model.parameters.borrow_mut().clear();
    }

    /// How many overrides are set.
    pub fn len(&self) -> usize {
        self.model.parameters.borrow().len()
    }

    /// Whether no overrides are set.
    pub fn is_empty(&self) -> bool {
        self.model.parameters.borrow().is_empty()
    }
}

/// The part opacities of one model, keyed by the interface's vocabulary.
///
/// Values are clamped to `0.0..=1.0` by the render layer, and pose fading applies
/// on top of them.
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
    pub(crate) fn id(&self) -> ModelId {
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

    pub(crate) fn options(&self) -> ModelOptions {
        self.options
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

    pub(crate) fn physics_enabled(&self) -> bool {
        self.physics_enabled.get()
    }

    pub(crate) fn parameter_indices(&self) -> Vec<(usize, f32)> {
        self.parameters
            .borrow()
            .iter()
            .map(|(key, value)| (key.index(), *value))
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

    fn asset() -> Live2dAsset {
        Live2dAsset::from_path("model.model3.json")
    }

    #[test]
    fn the_constructor_touches_no_files() {
        let asset = Live2dAsset::from_path("nowhere/gone.model3.json");

        assert_eq!(asset.path(), Path::new("nowhere/gone.model3.json"));
    }

    #[test]
    fn a_model_inherits_the_asset_identity_and_budget() {
        let asset = Live2dAsset::with_options(
            "model.model3.json",
            AssetOptions {
                texture_scale: 0.25,
                scan_sidecars: false,
            },
        );
        let model = Live2dModel::<Raw>::new(&asset);

        assert_eq!(model.asset(), asset.id());
        assert_eq!(model.path(), asset.path());
    }

    #[test]
    fn commands_keep_their_order_and_are_drained_once() {
        let model = Live2dModel::<Raw>::new(&asset());
        let group = RawMotion::from_id(&crate::interface::KeyId {
            asset: model.asset(),
            index: 0,
            name: Arc::from("Idle"),
        })
        .expect("a raw key");

        model.play_motion(group.clone(), 1);
        model.stop_motion();

        assert_eq!(model.inner.drain_commands().len(), 2);
        assert!(model.inner.drain_commands().is_empty());
    }

    #[test]
    fn the_queue_drops_the_oldest_command_and_reports_it_once() {
        let model = Live2dModel::<Raw>::new(&asset());
        let group = RawMotion::from_id(&crate::interface::KeyId {
            asset: model.asset(),
            index: 0,
            name: Arc::from("Idle"),
        })
        .expect("a raw key");

        for clip in 0..(MAX_QUEUED_COMMANDS + 6) {
            model.play_motion(group.clone(), clip);
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
        let model = Live2dModel::<Raw>::new(&own);
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
            Some(Problem::ForeignKey { vocabulary: "parameter", .. })
        ));
    }

    #[test]
    fn a_key_from_the_same_asset_is_accepted() {
        let asset = Live2dAsset::from_path("model.model3.json");
        let model = Live2dModel::<Raw>::new(&asset);
        let own = RawParameter::from_id(&crate::interface::KeyId {
            asset: model.asset(),
            index: 0,
            name: Arc::from("ParamAngleX"),
        })
        .expect("a raw key");

        assert_eq!(model.parameters().insert(own.clone(), 0.5), None);
        assert_eq!(model.parameters().get(&own), Some(0.5));
        assert_eq!(model.parameters().insert(own, 0.75), Some(0.5));

        let problems = model.inner.take_problems();
        assert!(problems.is_empty());
    }

    #[test]
    fn pause_and_physics_are_readable_back() {
        let model = Live2dModel::<Raw>::new(&asset());

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
        let model = Live2dModel::<Raw>::new(&asset());

        model.look_at([0.2, 0.3]);
        model.look_at([-0.5, 0.75]);

        assert_eq!(model.inner.target(), Some([-0.5, 0.75]));
    }

    #[test]
    fn an_untouched_model_has_no_target() {
        assert_eq!(Live2dModel::<Raw>::new(&asset()).inner.target(), None);
    }

    #[test]
    fn models_get_distinct_ids_and_presences() {
        let asset = asset();
        let first = Live2dModel::<Raw>::new(&asset);
        let second = Live2dModel::<Raw>::new(&asset);

        assert_ne!(first.inner.id(), second.inner.id());
        assert_ne!(
            Arc::as_ptr(&first.inner.presence()),
            Arc::as_ptr(&second.inner.presence())
        );
    }
}
