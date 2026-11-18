//! The model object a host owns.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use live2d_render::{LookAtMode, SceneOptions};

use crate::event::Problem;

/// How many commands may wait for the render layer before the oldest are dropped.
const MAX_QUEUED_COMMANDS: usize = 64;

/// Source of [`Live2dModel`] identities. The render layer keys its per-model
/// state on them, so they must not collide within a process.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// What the render layer may spend on one model.
///
/// This is this component's GPU budget, not the Cubism *model setting* (that is
/// `model3.json`). Start from the default and override what matters:
///
/// ```text
/// RenderSettings { texture_scale: 1.0, ..RenderSettings::default() }
/// ```
///
/// The default keeps a four-texture, 8192x8192 rig around 200 MB of VRAM, which
/// is what a desktop pet needs. `texture_scale` must be positive: a zero or
/// negative scale fails the load instead of guessing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSettings {
    /// Texture resolution multiplier applied when uploading.
    pub texture_scale: f32,
    /// Edge length of the shared mask render target. Raise it if a complex rig
    /// shows clipped geometry.
    pub mask_size: u32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        let options = SceneOptions::default();

        Self {
            texture_scale: options.texture_scale,
            mask_size: options.mask_size,
        }
    }
}

impl RenderSettings {
    pub(crate) fn to_scene_options(self) -> SceneOptions {
        SceneOptions {
            texture_scale: self.texture_scale,
            mask_size: self.mask_size,
            ..SceneOptions::default()
        }
    }
}

/// One model, owned by the host.
///
/// Build it with [`Live2dModel::from_path`] or [`Live2dModel::builder`], keep it
/// in the host's state and hand a `&Live2dModel` to the `live2d` widget. It is
/// `!Clone` and `!Send` on purpose: one owner, UI thread only.
///
/// Nothing happens at construction — no IO, no GPU. The first frame that draws
/// the model loads it, and the outcome arrives as
/// [`Live2dEvent::ModelLoaded`](crate::Live2dEvent::ModelLoaded) or
/// [`Live2dEvent::ModelFailedToLoad`](crate::Live2dEvent::ModelFailedToLoad).
/// A host that needs to react before then should track its own "still loading"
/// flag.
///
/// Dropping the model releases its GPU resources at the end of the next rendered
/// frame.
pub struct Live2dModel(Rc<Inner>);

struct Inner {
    id: u64,
    path: Arc<PathBuf>,
    settings: RenderSettings,
    look_at: Cell<Option<[f32; 2]>>,
    look_at_mode: Cell<LookAtMode>,
    paused: Cell<bool>,
    physics_enabled: Cell<bool>,
    commands: RefCell<VecDeque<Command>>,
    problems: RefCell<Vec<Problem>>,
    presence: Arc<Presence>,
}

/// Proof that a model is still alive: the render layer holds a `Weak` to this and
/// frees the model's GPU state once it can no longer be upgraded.
#[derive(Debug)]
pub(crate) struct Presence;

/// Something the host asked the render layer to do, queued until it can.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    PlayMotion(usize),
    PlayExpression(usize),
    StopMotion,
}

impl Live2dModel {
    /// A model to load from `path` with `settings`.
    ///
    /// `path` is the `.model3.json`; everything else is found from there. A path
    /// that does not exist is not rejected here — it fails on the first frame and
    /// is reported as a load failure.
    pub fn from_path(path: impl Into<PathBuf>, settings: RenderSettings) -> Self {
        Self::new(path, settings)
    }

    /// Names the same model one setting at a time:
    ///
    /// ```text
    /// Live2dModel::builder(path)
    ///     .settings(RenderSettings::default())
    ///     .texture_scale(1.0)
    ///     .build()
    /// ```
    ///
    /// The budget is required: a builder that invented one would hide which
    /// settings the model actually runs with. To change a single field, start
    /// from [`RenderSettings::default`] and use struct update syntax.
    pub fn builder(path: impl Into<PathBuf>) -> Live2dModelBuilder<NoSettings> {
        Live2dModelBuilder {
            path: path.into(),
            stage: NoSettings,
        }
    }

    fn new(path: impl Into<PathBuf>, settings: RenderSettings) -> Self {
        Self(Rc::new(Inner {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            path: Arc::new(path.into()),
            settings,
            look_at: Cell::new(None),
            look_at_mode: Cell::new(LookAtMode::default()),
            paused: Cell::new(false),
            physics_enabled: Cell::new(true),
            commands: RefCell::new(VecDeque::new()),
            problems: RefCell::new(Vec::new()),
            presence: Arc::new(Presence),
        }))
    }

    /// The file this model was created from.
    pub fn path(&self) -> &PathBuf {
        &self.0.path
    }

    /// The budget this model was created with.
    pub fn settings(&self) -> RenderSettings {
        self.0.settings
    }

    /// Points head, body and eyes at `target`, given in `-1.0..=1.0` on both
    /// axes (`-1` = left/top, `+y` = up).
    ///
    /// Which parameters that means is resolved per model. The value persists
    /// until it is set again, so a host without a cursor can call this once.
    ///
    /// While the widget has a [`LookAtSource`](crate::LookAtSource) and the
    /// cursor is over it, the source wins; this value is what the model returns
    /// to when the cursor leaves.
    pub fn look_at(&self, target: [f32; 2]) {
        self.0.look_at.set(Some(target));
    }

    /// Which parameters a LookAt target is written to.
    pub fn look_at_mode(&self) -> LookAtMode {
        self.0.look_at_mode.get()
    }

    /// Switches where a LookAt target lands, from the next frame on.
    ///
    /// Both modes are resolved from the model's own physics wiring, so switching
    /// never needs a reload.
    pub fn set_look_at_mode(&self, mode: LookAtMode) {
        self.0.look_at_mode.set(mode);
    }

    /// Stops time. The widget stops asking for frames, so the model holds its
    /// pose until [`Live2dModel::resume`].
    pub fn pause(&self) {
        self.0.paused.set(true);
    }

    /// Restarts time after [`Live2dModel::pause`].
    pub fn resume(&self) {
        self.0.paused.set(false);
    }

    /// Whether time is stopped.
    pub fn is_paused(&self) -> bool {
        self.0.paused.get()
    }

    /// Turns physics evaluation on or off, for a still frame or a diagnostic A/B.
    pub fn set_physics_enabled(&self, enabled: bool) {
        self.0.physics_enabled.set(enabled);
    }

    /// Whether physics is evaluated.
    pub fn physics_enabled(&self) -> bool {
        self.0.physics_enabled.get()
    }

    /// Plays the motion at `index`, in the order `motions` reports.
    ///
    /// The request is queued and takes effect on the next frame. An index the
    /// model does not have is reported as a
    /// [`Problem::Renderer`](crate::Problem::Renderer), not ignored.
    pub fn play_motion(&self, index: usize) {
        self.push_command(Command::PlayMotion(index));
    }

    /// Applies the expression at `index` on top of the motion.
    pub fn play_expression(&self, index: usize) {
        self.push_command(Command::PlayExpression(index));
    }

    /// Stops the current motion, revealing the rest pose.
    pub fn stop_motion(&self) {
        self.push_command(Command::StopMotion);
    }

    fn push_command(&self, command: Command) {
        let mut commands = self.0.commands.borrow_mut();

        if commands.len() >= MAX_QUEUED_COMMANDS {
            commands.pop_front();
            self.report_dropped_command();
        }

        commands.push_back(command);
    }

    fn report_dropped_command(&self) {
        let mut problems = self.0.problems.borrow_mut();

        for problem in problems.iter_mut() {
            if let Problem::CommandQueueOverflow { dropped } = problem {
                *dropped += 1;
                return;
            }
        }

        problems.push(Problem::CommandQueueOverflow { dropped: 1 });
    }

    pub(crate) fn id(&self) -> u64 {
        self.0.id
    }

    pub(crate) fn path_arc(&self) -> Arc<PathBuf> {
        Arc::clone(&self.0.path)
    }

    pub(crate) fn presence(&self) -> Arc<Presence> {
        Arc::clone(&self.0.presence)
    }

    /// The target most recently given to [`Live2dModel::look_at`].
    pub(crate) fn target(&self) -> Option<[f32; 2]> {
        self.0.look_at.get()
    }

    pub(crate) fn drain_commands(&self) -> Vec<Command> {
        let mut commands = self.0.commands.borrow_mut();
        commands.drain(..).collect()
    }

    pub(crate) fn take_problems(&self) -> Vec<Problem> {
        let mut problems = self.0.problems.borrow_mut();
        problems.drain(..).collect()
    }
}

impl fmt::Debug for Live2dModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Live2dModel")
            .field("id", &self.0.id)
            .field("path", &self.0.path)
            .field("settings", &self.0.settings)
            .finish_non_exhaustive()
    }
}

/// Names the settings of a model whose budget has not been given yet.
pub struct NoSettings;

/// Names the settings of a model whose budget has been given.
pub struct WithSettings(RenderSettings);

/// A model being described setting by setting.
///
/// The settings are required for [`Live2dModelBuilder::build`] to exist, so
/// forgetting them is a compile error rather than a silent default. Note that a
/// conditional budget (`if wide { .. } else { .. }`) cannot be built up in place:
/// the two branches have different types. Pass a value either way and override
/// fields on it.
pub struct Live2dModelBuilder<Stage> {
    path: PathBuf,
    stage: Stage,
}

impl Live2dModelBuilder<NoSettings> {
    /// Sets the budget. Every other setting is changed on top of this value.
    pub fn settings(self, settings: RenderSettings) -> Live2dModelBuilder<WithSettings> {
        Live2dModelBuilder {
            path: self.path,
            stage: WithSettings(settings),
        }
    }
}

impl Live2dModelBuilder<WithSettings> {
    /// Overrides the texture resolution multiplier.
    pub fn texture_scale(mut self, scale: f32) -> Self {
        self.stage.0.texture_scale = scale;
        self
    }

    /// Overrides the mask render-target edge length.
    pub fn mask_size(mut self, size: u32) -> Self {
        self.stage.0.mask_size = size;
        self
    }

    /// Creates the model. Nothing is loaded yet; see [`Live2dModel`].
    pub fn build(self) -> Live2dModel {
        Live2dModel::new(self.path, self.stage.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Live2dModel {
        Live2dModel::from_path("model.model3.json", RenderSettings::default())
    }

    #[test]
    fn the_constructor_touches_no_files() {
        let missing = Live2dModel::from_path("nowhere/gone.model3.json", RenderSettings::default());

        assert_eq!(missing.path(), &PathBuf::from("nowhere/gone.model3.json"));
    }

    #[test]
    fn the_builder_produces_the_same_model_as_from_path() {
        let settings = RenderSettings {
            texture_scale: 0.25,
            mask_size: 512,
        };
        let direct = Live2dModel::from_path("model.model3.json", settings);
        let built = Live2dModel::builder("model.model3.json")
            .settings(settings)
            .build();

        assert_eq!(direct.path(), built.path());
        assert_eq!(direct.settings(), built.settings());
    }

    #[test]
    fn the_builder_overrides_single_fields() {
        let settings = RenderSettings {
            texture_scale: 1.0,
            mask_size: 512,
        };
        let built = Live2dModel::builder("model.model3.json")
            .settings(RenderSettings::default())
            .texture_scale(1.0)
            .mask_size(512)
            .build();

        assert_eq!(built.settings(), settings);
    }

    #[test]
    fn commands_keep_their_order() {
        let model = model();

        model.play_motion(1);
        model.play_expression(2);
        model.stop_motion();

        assert_eq!(
            model.drain_commands(),
            vec![
                Command::PlayMotion(1),
                Command::PlayExpression(2),
                Command::StopMotion
            ]
        );
    }

    #[test]
    fn the_queue_drops_the_oldest_command_and_reports_it_once() {
        let model = model();

        for index in 0..(MAX_QUEUED_COMMANDS + 6) {
            model.play_motion(index);
        }

        let drained = model.drain_commands();

        assert_eq!(drained.len(), MAX_QUEUED_COMMANDS);
        assert_eq!(drained.first(), Some(&Command::PlayMotion(6)));

        let problems = model.take_problems();

        assert_eq!(problems.len(), 1);
        assert!(matches!(
            problems.first(),
            Some(Problem::CommandQueueOverflow { dropped: 6 })
        ));
    }

    #[test]
    fn draining_the_queue_empties_it() {
        let model = model();

        model.play_motion(3);

        assert_eq!(model.drain_commands(), vec![Command::PlayMotion(3)]);
        assert!(model.drain_commands().is_empty());
    }

    #[test]
    fn the_last_look_at_value_wins() {
        let model = model();

        model.look_at([0.2, 0.3]);
        model.look_at([-0.5, 0.75]);

        assert_eq!(model.target(), Some([-0.5, 0.75]));
    }

    #[test]
    fn an_untouched_model_has_no_target() {
        assert_eq!(model().target(), None);
    }

    #[test]
    fn pause_and_physics_are_readable_back() {
        let model = model();

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
    fn models_get_distinct_ids_and_presences() {
        let first = model();
        let second = model();

        assert_ne!(first.0.id, second.0.id);
        assert_ne!(
            Arc::as_ptr(&first.presence()),
            Arc::as_ptr(&second.presence())
        );
    }
}
