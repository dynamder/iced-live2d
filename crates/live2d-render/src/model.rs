//! One instance: its parameters, player, expressions and physics.
//!
//! Pure CPU state — no GPU handle, no `wgpu` call — so the whole update order can
//! be tested without a device.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use mocari::json::Motion3;
use mocari::moc3::Moc3DrawableMesh;
use mocari::motion::{DEFAULT_FADE_SECONDS, MotionPlayer, load_motion};
use mocari::{ExpressionManager, ModelRuntime};

use crate::asset::{MotionRef, Shared};
use crate::error::Error;
use crate::fade::Fade;
use crate::index::{
    ExpressionIndex, HitAreaIndex, MotionClipIndex, MotionGroupIndex, ParameterIndex, PartIndex,
};
use crate::info::ModelInfo;
use crate::policy::{LookAtBlend, ParameterPolicy, ParameterStage, ParameterWrite};
use crate::rig::LookAtMode;

/// The animation step a model advances in, whatever the host's frame rate: 60 Hz is
/// what Live2D models are authored against.
///
/// Pass this as the interval to [`Model::advance`]. A rounded one (16 ms) drifts
/// below real time, because only whole steps are spent.
pub const STEP: Duration = Duration::from_micros(16_667);

/// Longest interval one `advance` call may count, so a stalled window does not
/// teleport the motion timeline.
const MAX_CALL: Duration = Duration::from_millis(100);

/// Time the model may bank and spend on steps, i.e. how far it may fall behind
/// before it starts dropping time instead of catching up.
const MAX_CARRY: Duration = STEP.saturating_mul(4);

/// The seed of the per-model random clip picker.
const RANDOM_SEED: u64 = 0x2545_F491_4F6C_DD1D;

/// A motion that has just started playing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Started {
    /// The motion group that started.
    pub group: MotionGroupIndex,
    /// The clip inside that group.
    pub clip: MotionClipIndex,
    /// Length of the clip in seconds, as declared by the motion file.
    pub duration: f32,
    /// Whether the file asks for looping.
    pub looping: bool,
}

/// A motion that stopped being the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ended {
    /// The motion group that stopped.
    pub group: MotionGroupIndex,
    /// The clip inside that group.
    pub clip: MotionClipIndex,
    /// Whether it reached its own end or was stopped.
    pub reason: MotionEnd,
}

/// Why a motion stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionEnd {
    /// It reached its own end.
    Completed,
    /// It was replaced or stopped before that.
    Interrupted,
}

/// One model instance, built with [`Asset::model`](crate::Asset::model).
///
/// [`Model::advance`] is the only place parameters are written:
/// `reset → motion → expression → look-at → host (before physics) → parts →
/// physics → host (after physics) → pose`. A host write therefore wins over the
/// motion, and [`ParameterPolicy`] decides whether the simulation sees it and how
/// it combines with what played.
pub struct Model {
    shared: Shared,
    runtime: ModelRuntime,
    player: Option<MotionPlayer>,
    /// Motions that were interrupted and are still fading out, oldest first — the
    /// order they applied in while they were live.
    fading: Vec<MotionPlayer>,
    current: Option<(MotionGroupIndex, MotionClipIndex)>,
    expressions: ExpressionManager,
    parameters: Vec<(ParameterIndex, ParameterWrite)>,
    parameter_policy: ParameterPolicy,
    look_at_blend: LookAtBlend,
    parts: Vec<(PartIndex, f32)>,
    look_at: Option<[f32; 2]>,
    look_at_mode: LookAtMode,
    physics_enabled: bool,
    random: u64,
    dirty: bool,
    stepper: Stepper,
    started: Vec<Started>,
    ended: Vec<Ended>,
    /// `None` leaves the fade to the files.
    fade: Option<Fade>,
    /// `None` lets every interrupted motion fade out fully.
    fade_limit: Option<usize>,
    /// Parameter names already reported as overwritten by physics.
    reported_overwrites: BTreeSet<Arc<str>>,
    /// Positions already reported as parameters the model does not declare.
    reported_parameters: BTreeSet<ParameterIndex>,
    /// Positions already reported as parts the model does not declare.
    reported_parts: BTreeSet<PartIndex>,
    issues: Vec<Error>,
}

impl Model {
    pub(crate) fn new(runtime: ModelRuntime, shared: Shared) -> Self {
        Self {
            shared,
            runtime,
            player: None,
            fading: Vec::new(),
            current: None,
            expressions: ExpressionManager::new(),
            parameters: Vec::new(),
            parameter_policy: ParameterPolicy::default(),
            look_at_blend: LookAtBlend::default(),
            parts: Vec::new(),
            look_at: None,
            look_at_mode: LookAtMode::default(),
            physics_enabled: true,
            random: RANDOM_SEED,
            dirty: true,
            stepper: Stepper::new(),
            started: Vec::new(),
            ended: Vec::new(),
            fade: None,
            fade_limit: None,
            reported_overwrites: BTreeSet::new(),
            reported_parameters: BTreeSet::new(),
            reported_parts: BTreeSet::new(),
            issues: Vec::new(),
        }
    }

    /// What the model declares: names, ranges, hit areas.
    pub fn info(&self) -> &Arc<ModelInfo> {
        &self.shared.info
    }

    /// Advances the animation by `delta`, returning how many steps ran.
    ///
    /// Zero means the pose did not change, so there is nothing to
    /// [`Model::deform`] or re-upload. Time is spent in whole [`STEP`]s and the
    /// remainder kept, so the pose depends on elapsed time alone. More than four
    /// steps of backlog are dropped, not sprinted through.
    pub fn advance(&mut self, delta: Duration) -> u32 {
        let steps = self.stepper.consume(delta);

        if !self.is_animating() {
            return 0;
        }

        for _ in 0..steps {
            self.step();
        }

        steps
    }

    /// Rebuilds every drawable from the current parameters, parts and pose.
    ///
    /// `false` means the data could not produce a valid mesh update; the previous
    /// geometry stays on screen.
    pub fn deform(&mut self) -> bool {
        self.dirty = false;
        self.runtime.update_meshes().is_some()
    }

    /// The current drawable meshes, in model order.
    pub fn meshes(&self) -> &[Moc3DrawableMesh] {
        self.runtime.meshes()
    }

    /// The host writes to replay every step, as `(parameter, write)` pairs.
    /// Setting an identical slice again is free. A parameter the model does not
    /// declare is reported once as an issue and never written.
    pub fn set_parameters(&mut self, writes: &[(ParameterIndex, ParameterWrite)]) {
        if self.parameters == writes {
            return;
        }

        self.parameters.clear();
        self.parameters.extend_from_slice(writes);
        self.report_unknown_parameters();
        self.dirty = true;
    }

    /// How writes that do not name their own blend or stage are applied.
    pub fn parameter_policy(&self) -> ParameterPolicy {
        self.parameter_policy
    }

    /// Changes how unnamed writes are applied from the next step on.
    pub fn set_parameter_policy(&mut self, policy: ParameterPolicy) {
        if self.parameter_policy == policy {
            return;
        }

        self.parameter_policy = policy;
        self.dirty = true;
    }

    /// How the built-in look-at combines with what the model plays.
    pub fn look_at_blend(&self) -> LookAtBlend {
        self.look_at_blend
    }

    /// Changes how the built-in look-at combines with the pose.
    pub fn set_look_at_blend(&mut self, blend: LookAtBlend) {
        if self.look_at_blend == blend {
            return;
        }

        self.look_at_blend = blend;
        self.dirty = true;
    }

    /// The part opacities to replay every step, as `(part, opacity)` pairs, clamped
    /// to `0.0..=1.0`. Pose fading applies on top. A part the model does not
    /// declare is reported once as an issue and never written.
    pub fn set_parts(&mut self, overrides: &[(PartIndex, f32)]) {
        if self.parts == overrides {
            return;
        }

        self.parts.clear();
        self.parts.extend_from_slice(overrides);
        self.report_unknown_parts();
        self.dirty = true;
    }

    /// Points the model at a target in `-1.0..=1.0` on both axes (`+y` up), or
    /// leaves it to its own motion and physics when `None`.
    ///
    /// Where the target lands is fixed by [`Model::set_look_at_mode`].
    pub fn set_look_at(&mut self, target: Option<[f32; 2]>) {
        if self.look_at == target {
            return;
        }

        self.look_at = target;
        self.dirty = true;
    }

    /// Which table a LookAt target is written to.
    pub fn look_at_mode(&self) -> LookAtMode {
        self.look_at_mode
    }

    /// Switches where a LookAt target lands. Both tables are resolved at load time,
    /// so this cannot fail and touches no other input.
    pub fn set_look_at_mode(&mut self, mode: LookAtMode) {
        if self.look_at_mode == mode {
            return;
        }

        self.look_at_mode = mode;
        self.dirty = true;
    }

    /// Whether physics is evaluated.
    pub fn physics_enabled(&self) -> bool {
        self.physics_enabled
    }

    /// Turns physics evaluation on or off, for a still frame or a diagnostic A/B.
    pub fn set_physics_enabled(&mut self, enabled: bool) {
        if self.physics_enabled == enabled {
            return;
        }

        self.physics_enabled = enabled;
        self.dirty = true;
    }

    /// Plays `clip` of `group`, loading the clip now — the only file read this call
    /// can cause. An out-of-range pair becomes an issue, not a silent no-op.
    ///
    /// Whatever was playing fades out over its own fade-out time while this one fades
    /// in, which is what the Cubism viewer does; [`Model::set_fade`] sets the value
    /// used when nothing in the model folder declares one.
    pub fn play_motion(&mut self, group: MotionGroupIndex, clip: MotionClipIndex) {
        let reference = match clip_ref(&self.shared.info, &self.shared.motions, group, clip) {
            Ok(reference) => reference.clone(),
            Err(error) => {
                self.issues.push(error);
                return;
            }
        };

        match load_motion(&reference.path) {
            Ok(motion) => self.start(group, clip, motion, &reference),
            Err(source) => self.issues.push(Error::Motion {
                path: reference.path,
                source,
            }),
        }
    }

    /// Plays a random clip of `group`.
    pub fn play_random_motion(&mut self, group: MotionGroupIndex) {
        let draw = self.next_random();

        match pick_clip(&self.shared.info, group, draw) {
            Ok(clip) => self.play_motion(group, clip),
            Err(error) => self.issues.push(error),
        }
    }

    /// Applies an expression on top of the motion. A position the model does not
    /// declare is reported as an issue, not a silent no-op.
    pub fn play_expression(&mut self, expression: ExpressionIndex) {
        let Some(path) = self.shared.expressions.get(expression.get()).cloned() else {
            let count = self.shared.expressions.len();
            self.issues.push(Error::UnknownExpression {
                index: expression,
                count,
            });
            return;
        };

        match mocari::expression::load_expression(&path.path) {
            Ok(expression) => {
                self.expressions.play(expression);
                self.dirty = true;
            }
            Err(source) => self.issues.push(Error::Expression {
                path: path.path,
                source,
            }),
        }
    }

    /// Drops the expressions currently applied, fading them out.
    pub fn clear_expressions(&mut self) {
        if self.expressions.is_empty() {
            return;
        }

        self.expressions.stop_all();
        self.dirty = true;
    }

    /// Stops the current motion, revealing the rest pose over its own fade-out time.
    /// Nothing plays afterwards until the host asks: there is no idle policy here.
    pub fn stop_motion(&mut self) {
        let Some((group, clip)) = self.current.take() else {
            return;
        };

        if let Some(mut player) = self.player.take()
            && player.fade_out()
        {
            self.fading.push(player);
        }

        self.ended.push(Ended {
            group,
            clip,
            reason: MotionEnd::Interrupted,
        });
        self.dirty = true;
    }

    /// How long a motion fades when nothing in the model folder declares a time.
    ///
    /// A motion that declares `FadeInTime` / `FadeOutTime`, or whose `model3.json`
    /// reference does, keeps that value: the files are the artist's decision. `None`
    /// — the default — leaves the files alone and gives the gap
    /// [`Fade::default`], Cubism's one second.
    pub fn set_fade(&mut self, fade: Option<Fade>) {
        if self.fade == fade {
            return;
        }

        self.fade = fade;
        self.dirty = true;
    }

    /// The fade a host set, if any.
    pub fn fade(&self) -> Option<Fade> {
        self.fade
    }

    /// Caps how many interrupted motions may still be fading at once.
    ///
    /// `None`, the default, lets every one of them run to its own end; a number drops
    /// the oldest, which is the one closest to invisible.
    pub fn set_fade_limit(&mut self, limit: Option<usize>) {
        self.fade_limit = limit;

        if let Some(limit) = limit {
            self.trim_fading(limit);
        }
    }

    /// The cap on concurrent fade-outs, if any.
    pub fn fade_limit(&self) -> Option<usize> {
        self.fade_limit
    }

    /// Motions that started since the last call, in the order they started.
    pub fn take_started(&mut self) -> Vec<Started> {
        std::mem::take(&mut self.started)
    }

    /// Motions that stopped since the last call, in order, with the reason. This is
    /// how a host learns to ask for the next one, or that its clip was replaced.
    pub fn take_ended(&mut self) -> Vec<Ended> {
        std::mem::take(&mut self.ended)
    }

    /// Problems that did not stop playback, such as a broken sidecar file.
    pub fn take_issues(&mut self) -> Vec<Error> {
        std::mem::take(&mut self.issues)
    }

    /// The first hit area containing a model-space point.
    pub fn hit_area(&self, point: [f32; 2]) -> Option<HitAreaIndex> {
        let hit = self.runtime.hit_test(point[0], point[1])?;
        self.shared.info.hit_area_of_drawable(hit.id())
    }

    fn is_animating(&self) -> bool {
        self.dirty
            || self.player.is_some()
            || !self.fading.is_empty()
            || !self.expressions.is_empty()
            || (self.physics_enabled && self.runtime.physics().is_some())
    }

    /// Drops the oldest fade-outs past `limit`.
    fn trim_fading(&mut self, limit: usize) {
        if self.fading.len() > limit {
            self.fading.drain(..self.fading.len() - limit);
        }
    }

    /// One fixed-length step through the update order.
    fn step(&mut self) {
        let seconds = STEP.as_secs_f32();

        self.runtime.reset_parameters();
        self.runtime.reset_part_opacities();

        self.play_hand_overs(seconds);

        if let Some(player) = self.player.as_mut() {
            player.tick(seconds);
            player.apply(&mut self.runtime);
        }

        self.expressions.tick(seconds);
        self.expressions.apply(&mut self.runtime);

        self.apply_look_at();
        self.apply_writes(ParameterStage::BeforePhysics);

        for &(index, opacity) in &self.parts {
            self.runtime.set_part_opacity_by_index(index.get(), opacity);
        }

        if self.physics_enabled {
            self.runtime.apply_physics(seconds);
        }

        self.apply_writes(ParameterStage::AfterPhysics);
        self.runtime.apply_pose(seconds);

        if self.player.as_ref().is_some_and(MotionPlayer::is_finished) {
            if let Some((group, clip)) = self.current.take() {
                self.ended.push(Ended {
                    group,
                    clip,
                    reason: MotionEnd::Completed,
                });
            }

            self.player = None;
        }
    }

    /// Plays every interrupted motion for one more step, oldest first, and drops the
    /// ones that have finished fading.
    ///
    /// Oldest first is the order they applied in while they were live, so a burst of
    /// switches layers the way the Cubism viewer does.
    fn play_hand_overs(&mut self, seconds: f32) {
        if self.fading.is_empty() {
            return;
        }

        for player in &mut self.fading {
            player.tick(seconds);
            player.apply(&mut self.runtime);
        }

        self.fading.retain(|player| !player.is_finished());
    }

    fn apply_writes(&mut self, stage: ParameterStage) {
        for index in 0..self.parameters.len() {
            let (parameter, write) = self.parameters[index];

            if self.write_stage(parameter, write) != stage {
                continue;
            }

            if stage == ParameterStage::BeforePhysics {
                self.report_physics_override(parameter);
            }

            let Some(current) = self.runtime.parameter_value_by_index(parameter.get()) else {
                continue;
            };

            self.runtime.set_parameter_by_index(
                parameter.get(),
                write.apply(current, self.parameter_policy),
            );
        }
    }

    fn write_stage(&self, parameter: ParameterIndex, write: ParameterWrite) -> ParameterStage {
        write.resolved_stage(
            self.parameter_policy,
            self.shared.facts.is_physics_output(parameter),
        )
    }

    fn report_physics_override(&mut self, parameter: ParameterIndex) {
        if !self.physics_enabled || !self.shared.facts.is_physics_output(parameter) {
            return;
        }

        let Some(name) = self.shared.facts.name(parameter) else {
            return;
        };

        let name: Arc<str> = Arc::from(name);

        if !self.reported_overwrites.insert(name.clone()) {
            return;
        }

        self.issues.push(Error::ParameterOverwritten {
            name: name.to_string(),
        });
    }

    fn report_unknown_parameters(&mut self) {
        let count = self.shared.info.parameters.len();

        for &(parameter, _) in &self.parameters {
            if parameter.get() < count || !self.reported_parameters.insert(parameter) {
                continue;
            }

            self.issues.push(Error::UnknownParameter {
                index: parameter,
                count,
            });
        }
    }

    fn report_unknown_parts(&mut self) {
        let count = self.shared.info.parts.len();

        for &(part, _) in &self.parts {
            if part.get() < count || !self.reported_parts.insert(part) {
                continue;
            }

            self.issues.push(Error::UnknownPart { index: part, count });
        }
    }

    fn apply_look_at(&mut self) {
        let Some(target) = self.look_at else {
            return;
        };

        let rig = self.shared.rigs.get(self.look_at_mode);

        for binding in rig.bindings() {
            let offset = binding.slot.axis().of(target).clamp(-1.0, 1.0);

            match self.look_at_blend {
                LookAtBlend::Overwrite => {
                    let amount = (offset + 1.0) * 0.5;

                    self.runtime
                        .set_parameter_normalized_by_index(binding.parameter.get(), amount);
                }
                LookAtBlend::Add => {
                    let (Some(minimum), Some(maximum)) = (
                        self.runtime
                            .parameter_minimum_by_index(binding.parameter.get()),
                        self.runtime
                            .parameter_maximum_by_index(binding.parameter.get()),
                    ) else {
                        continue;
                    };

                    let Some(current) = self
                        .runtime
                        .parameter_value_by_index(binding.parameter.get())
                    else {
                        continue;
                    };

                    self.runtime.set_parameter_by_index(
                        binding.parameter.get(),
                        current + offset * (maximum - minimum) * 0.5,
                    );
                }
            }
        }
    }

    fn start(
        &mut self,
        group: MotionGroupIndex,
        clip: MotionClipIndex,
        motion: Motion3,
        reference: &MotionRef,
    ) {
        self.started.push(Started {
            group,
            clip,
            duration: motion.meta().duration(),
            looping: motion.meta().is_looping(),
        });

        if let Some(mut player) = self.player.take() {
            if let Some((was, was_clip)) = self.current.take() {
                self.ended.push(Ended {
                    group: was,
                    clip: was_clip,
                    reason: MotionEnd::Interrupted,
                });
            }

            if player.fade_out() {
                self.fading.push(player);

                if let Some(limit) = self.fade_limit {
                    self.trim_fading(limit);
                }
            }
        }

        let meta = motion.meta();
        let fade_in = resolve_fade(self.fade, reference.fade_in, meta.fade_in_time());
        let fade_out = resolve_fade(self.fade, reference.fade_out, meta.fade_out_time());

        self.player = Some(MotionPlayer::with_fade(motion, fade_in, fade_out));
        self.current = Some((group, clip));
        self.dirty = true;
    }

    /// xorshift64: deterministic per model, good enough to pick a clip.
    fn next_random(&mut self) -> u32 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        (self.random >> 32) as u32
    }

    /// The motion currently playing, as `(group, clip)`.
    pub fn current_motion(&self) -> Option<(MotionGroupIndex, MotionClipIndex)> {
        self.current
    }

    /// A raw parameter value. Diagnostics and tests only.
    pub fn parameter_value(&self, id: &str) -> Option<f32> {
        self.runtime.parameter_value(id)
    }

    /// A raw parameter value by model index. Diagnostics and tests only.
    pub fn parameter_value_by_index(&self, index: ParameterIndex) -> Option<f32> {
        self.runtime.parameter_value_by_index(index.get())
    }

    /// Writes a raw parameter value. Diagnostics and tests only; hosts write
    /// through [`Model::set_parameters`]. `false` means the model declares no such
    /// parameter.
    pub fn set_parameter_raw(&mut self, index: ParameterIndex, value: f32) -> bool {
        self.runtime.set_parameter_by_index(index.get(), value)
    }
}

/// How many clips `group` holds, or the error that names what is missing.
fn group_clips(info: &ModelInfo, group: MotionGroupIndex) -> Result<usize, Error> {
    info.motion_clips(group).ok_or(Error::UnknownMotionGroup {
        group,
        groups: info.motions.len(),
    })
}

fn clip_ref<'a>(
    info: &ModelInfo,
    motions: &'a [Arc<[MotionRef]>],
    group: MotionGroupIndex,
    clip: MotionClipIndex,
) -> Result<&'a MotionRef, Error> {
    let clips = group_clips(info, group)?;

    motions
        .get(group.get())
        .and_then(|clips| clips.get(clip.get()))
        .ok_or(Error::UnknownMotionClip { group, clip, clips })
}

fn pick_clip(
    info: &ModelInfo,
    group: MotionGroupIndex,
    draw: u32,
) -> Result<MotionClipIndex, Error> {
    let clips = group_clips(info, group)?;

    if clips == 0 {
        return Err(Error::EmptyMotionGroup { group });
    }

    Ok(MotionClipIndex::new(draw as usize % clips))
}

/// Explicit setting, then the `.model3.json` reference, then the motion's own `Meta`,
/// then Cubism's one-second default.
fn resolve_fade(explicit: Option<Fade>, reference: Option<f32>, declared: Option<f32>) -> f32 {
    explicit
        .map(Fade::seconds)
        .or(reference)
        .or(declared)
        .unwrap_or(DEFAULT_FADE_SECONDS)
}

/// Fixed-timestep accumulator between the host's frame pacing and the model.
#[derive(Debug, Clone)]
struct Stepper {
    carry: Duration,
}

impl Stepper {
    /// Starts one step in debt so the first `advance` renders the loaded model
    /// instead of waiting a frame for time to accumulate.
    fn new() -> Self {
        Self { carry: STEP }
    }

    /// Takes `delta`, returns how many steps are due now.
    fn consume(&mut self, delta: Duration) -> u32 {
        self.carry = (self.carry + delta.clamp(Duration::ZERO, MAX_CALL)).min(MAX_CARRY);

        let steps = (self.carry.as_nanos() / STEP.as_nanos()) as u32;
        self.carry -= STEP.saturating_mul(steps);

        steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(clips: &[usize]) -> ModelInfo {
        ModelInfo {
            asset: crate::AssetId::unique(),
            bounds: crate::ModelBounds::try_new([0.0, 0.0], [1.0, 1.0]).expect("a box"),
            parameters: Arc::from([]),
            parts: Arc::from([]),
            motions: clips
                .iter()
                .map(|&clips| crate::MotionGroup {
                    name: Arc::from("group"),
                    clips,
                })
                .collect(),
            expressions: Arc::from([]),
            hit_areas: Arc::from([]),
        }
    }

    #[test]
    fn an_unknown_group_reports_how_many_groups_exist() {
        assert!(matches!(
            clip_ref(
                &info(&[1, 1]),
                &[],
                MotionGroupIndex::new(2),
                MotionClipIndex::new(0)
            ),
            Err(Error::UnknownMotionGroup { groups: 2, .. })
        ));
    }

    #[test]
    fn an_unknown_clip_reports_how_many_the_group_holds() {
        assert!(matches!(
            clip_ref(
                &info(&[3]),
                &[],
                MotionGroupIndex::new(0),
                MotionClipIndex::new(3)
            ),
            Err(Error::UnknownMotionClip { clips: 3, .. })
        ));
    }

    #[test]
    fn an_empty_group_is_not_an_unknown_one() {
        assert!(matches!(
            pick_clip(&info(&[0]), MotionGroupIndex::new(0), 0),
            Err(Error::EmptyMotionGroup { .. })
        ));
    }

    #[test]
    fn an_unknown_group_is_reported_before_the_clip() {
        assert!(matches!(
            clip_ref(
                &info(&[3]),
                &[],
                MotionGroupIndex::new(1),
                MotionClipIndex::new(9)
            ),
            Err(Error::UnknownMotionGroup { .. })
        ));
    }

    #[test]
    fn a_random_pick_stays_inside_the_group() {
        let info = info(&[3]);

        for draw in [0, 1, 2, 3, 7, u32::MAX] {
            let clip = pick_clip(&info, MotionGroupIndex::new(0), draw).expect("a clip");

            assert!(clip.get() < 3);
        }
    }

    /// Most steps a `Stepper` can hand out across the given intervals.
    fn steps_over(deltas: &[Duration]) -> u32 {
        let mut stepper = Stepper::new();
        deltas.iter().map(|delta| stepper.consume(*delta)).sum()
    }

    #[test]
    fn the_first_call_steps_the_loaded_model() {
        assert_eq!(steps_over(&[Duration::ZERO]), 1);
    }

    #[test]
    fn zero_length_frames_never_step() {
        let mut stepper = Stepper::new();
        stepper.consume(Duration::ZERO);

        for _ in 0..10 {
            assert_eq!(stepper.consume(Duration::ZERO), 0);
        }
    }

    #[test]
    fn one_frame_interval_is_one_step() {
        let mut stepper = Stepper::new();
        stepper.consume(Duration::ZERO);

        assert_eq!(stepper.consume(STEP), 1);
        assert_eq!(stepper.consume(STEP), 1);
    }

    #[test]
    fn leftover_time_carries_over() {
        let half = STEP / 2;
        let mut stepper = Stepper::new();
        stepper.consume(Duration::ZERO);

        assert_eq!(stepper.consume(half), 0);
        assert_eq!(stepper.consume(half), 1);
    }

    #[test]
    fn a_slow_host_runs_several_steps_per_call() {
        assert_eq!(steps_over(&[Duration::ZERO, STEP * 2]), 3);
    }

    #[test]
    fn steps_follow_elapsed_time_not_frame_count() {
        let quarter = STEP / 4;
        let chunked = steps_over(&[quarter; 8]);
        let whole = steps_over(&[quarter * 8]);

        assert_eq!(chunked, whole);
        assert_eq!(whole, 3);
    }

    #[test]
    fn a_stall_does_not_bank_time() {
        let mut stepper = Stepper::new();
        stepper.consume(Duration::ZERO);

        assert_eq!(stepper.consume(Duration::from_secs(10)), 4);
        assert_eq!(stepper.consume(Duration::ZERO), 0);
        assert_eq!(stepper.consume(Duration::ZERO), 0);
    }

    #[test]
    fn the_random_picker_moves_on() {
        let mut random = RANDOM_SEED;
        let mut picks = Vec::new();

        for _ in 0..8 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            picks.push((random >> 32) as u32 % 9);
        }

        assert!(picks.iter().any(|pick| *pick != picks[0]));
    }
}

#[cfg(test)]
mod fade_tests {
    use super::*;

    #[test]
    fn the_motion_file_decides_when_nothing_overrides_it() {
        assert_eq!(resolve_fade(None, None, Some(0.25)), 0.25);
    }

    #[test]
    fn the_model_file_reference_beats_the_motion_file() {
        assert_eq!(resolve_fade(None, Some(0.5), Some(0.25)), 0.5);
    }

    #[test]
    fn an_explicit_fade_beats_both_files() {
        let explicit = Fade::from(Duration::from_millis(120));

        assert_eq!(resolve_fade(Some(explicit), Some(0.5), Some(0.25)), 0.12);
    }

    #[test]
    fn a_declared_zero_survives_the_chain() {
        assert_eq!(resolve_fade(None, None, Some(0.0)), 0.0);
        assert_eq!(resolve_fade(Some(Fade::zero()), Some(0.5), None), 0.0);
    }

    #[test]
    fn nothing_declared_means_the_sdk_default() {
        assert_eq!(resolve_fade(None, None, None), DEFAULT_FADE_SECONDS);
    }
}
