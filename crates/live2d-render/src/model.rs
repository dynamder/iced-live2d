//! One instance: its parameters, player, expressions and physics.
//!
//! Pure CPU state — no GPU handle, no `wgpu` call — so the whole update order can
//! be tested without a device.

use std::sync::Arc;
use std::time::Duration;

use mocari::json::Motion3;
use mocari::moc3::Moc3DrawableMesh;
use mocari::motion::{MotionPlayer, load_motion};
use mocari::{ExpressionManager, ModelRuntime};

use crate::asset::Shared;
use crate::error::Error;
use crate::info::ModelInfo;
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
    /// Model index of the motion group.
    pub group: usize,
    /// Index of the clip inside that group.
    pub clip: usize,
    /// Length of the clip in seconds, as declared by the motion file.
    pub duration: f32,
    /// Whether the file asks for looping.
    pub looping: bool,
}

/// A motion that reached its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finished {
    /// Model index of the motion group.
    pub group: usize,
    /// Index of the clip inside that group.
    pub clip: usize,
}

/// One model instance, built with [`Asset::model`](crate::Asset::model).
///
/// [`Model::advance`] is the only place parameters are written, in Cubism's order:
/// `reset → motion → expression → host parameters → look-at → parts → physics →
/// pose`. A host parameter therefore wins over the motion, and physics sees the
/// final input.
pub struct Model {
    shared: Shared,
    runtime: ModelRuntime,
    player: Option<MotionPlayer>,
    current: Option<(usize, usize)>,
    expressions: ExpressionManager,
    parameters: Vec<(usize, f32)>,
    parts: Vec<(usize, f32)>,
    look_at: Option<[f32; 2]>,
    look_at_mode: LookAtMode,
    physics_enabled: bool,
    random: u64,
    dirty: bool,
    stepper: Stepper,
    started: Vec<Started>,
    finished: Vec<Finished>,
    issues: Vec<Error>,
}

impl Model {
    pub(crate) fn new(runtime: ModelRuntime, shared: Shared) -> Self {
        Self {
            shared,
            runtime,
            player: None,
            current: None,
            expressions: ExpressionManager::new(),
            parameters: Vec::new(),
            parts: Vec::new(),
            look_at: None,
            look_at_mode: LookAtMode::default(),
            physics_enabled: true,
            random: RANDOM_SEED,
            dirty: true,
            stepper: Stepper::new(),
            started: Vec::new(),
            finished: Vec::new(),
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

    /// The raw parameter overrides to replay every step, as `(model index, raw
    /// value)` pairs. Setting an identical slice again is free.
    pub fn set_parameters(&mut self, overrides: &[(usize, f32)]) {
        if self.parameters == overrides {
            return;
        }

        self.parameters.clear();
        self.parameters.extend_from_slice(overrides);
        self.dirty = true;
    }

    /// The part opacities to replay every step, as `(model index, opacity)` pairs,
    /// clamped to `0.0..=1.0`. Pose fading applies on top.
    pub fn set_parts(&mut self, overrides: &[(usize, f32)]) {
        if self.parts == overrides {
            return;
        }

        self.parts.clear();
        self.parts.extend_from_slice(overrides);
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
    pub fn play_motion(&mut self, group: usize, clip: usize) {
        let Some(path) = self
            .shared
            .motions
            .get(group)
            .and_then(|clips| clips.get(clip))
            .cloned()
        else {
            let count = self.shared.info.motion_clips(group);

            self.issues.push(match self.shared.motions.get(group) {
                Some(_) => Error::MotionClip { group, clip, count },
                None => Error::MotionGroup { group, count },
            });
            return;
        };

        match load_motion(&path) {
            Ok(motion) => self.start(group, clip, motion),
            Err(source) => self.issues.push(Error::Motion { path, source }),
        }
    }

    /// Plays a random clip of `group`.
    pub fn play_random_motion(&mut self, group: usize) {
        let count = self.shared.info.motion_clips(group);

        if count == 0 {
            self.issues.push(Error::MotionClip {
                group,
                clip: 0,
                count,
            });
            return;
        }

        let clip = (self.next_random() as usize) % count;
        self.play_motion(group, clip);
    }

    /// Applies an expression on top of the motion.
    pub fn play_expression(&mut self, expression: usize) {
        let Some(path) = self.shared.expressions.get(expression).cloned() else {
            let count = self.shared.expressions.len();
            self.issues.push(Error::ExpressionIndex {
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

    /// Stops the current motion so the model shows its rest pose. Nothing plays
    /// afterwards until the host asks: there is no idle policy here.
    pub fn stop_motion(&mut self) {
        if self.player.is_none() {
            return;
        }

        self.player = None;
        self.current = None;
        self.dirty = true;
    }

    /// Motions that started since the last call, in the order they started.
    pub fn take_started(&mut self) -> Vec<Started> {
        std::mem::take(&mut self.started)
    }

    /// Motions that ended since the last call, in order. Playback stops there; this
    /// is how a host learns to ask for the next one.
    pub fn take_finished(&mut self) -> Vec<Finished> {
        std::mem::take(&mut self.finished)
    }

    /// Problems that did not stop playback, such as a broken sidecar file.
    pub fn take_issues(&mut self) -> Vec<Error> {
        std::mem::take(&mut self.issues)
    }

    /// The first hit area containing a model-space point.
    pub fn hit_area(&self, point: [f32; 2]) -> Option<usize> {
        let hit = self.runtime.hit_test(point[0], point[1])?;
        self.shared.info.hit_area_of_drawable(hit.id())
    }

    fn is_animating(&self) -> bool {
        self.dirty
            || self.player.is_some()
            || !self.expressions.is_empty()
            || (self.physics_enabled && self.runtime.physics().is_some())
    }

    /// One fixed-length step through the update order.
    fn step(&mut self) {
        let seconds = STEP.as_secs_f32();

        self.runtime.reset_parameters();
        self.runtime.reset_part_opacities();

        if let Some(player) = self.player.as_mut() {
            player.tick(seconds);
            player.apply(&mut self.runtime);
        }

        self.expressions.tick(seconds);
        self.expressions.apply(&mut self.runtime);

        for &(index, value) in &self.parameters {
            self.runtime.set_parameter_by_index(index, value);
        }

        self.apply_look_at();

        for &(index, opacity) in &self.parts {
            self.runtime.set_part_opacity_by_index(index, opacity);
        }

        if self.physics_enabled {
            self.runtime.apply_physics(seconds);
        }

        self.runtime.apply_pose(seconds);

        if self.player.as_ref().is_some_and(MotionPlayer::is_finished) {
            if let Some((group, clip)) = self.current.take() {
                self.finished.push(Finished { group, clip });
            }

            self.player = None;
        }
    }

    fn apply_look_at(&mut self) {
        let Some(target) = self.look_at else {
            return;
        };

        let rig = self.shared.rigs.get(self.look_at_mode);

        for binding in rig.bindings() {
            let offset = binding.slot.axis().of(target);
            let amount = (offset.clamp(-1.0, 1.0) + 1.0) * 0.5;

            self.runtime
                .set_parameter_normalized_by_index(binding.parameter, amount);
        }
    }

    fn start(&mut self, group: usize, clip: usize, motion: Motion3) {
        self.started.push(Started {
            group,
            clip,
            duration: motion.meta().duration(),
            looping: motion.meta().is_looping(),
        });

        self.player = Some(MotionPlayer::new(motion));
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

    /// The index of the motion currently playing, as `(group, clip)`.
    pub fn current_motion(&self) -> Option<(usize, usize)> {
        self.current
    }

    /// A raw parameter value. Diagnostics and tests only.
    pub fn parameter_value(&self, id: &str) -> Option<f32> {
        self.runtime.parameter_value(id)
    }

    /// A raw parameter value by model index. Diagnostics and tests only.
    pub fn parameter_value_by_index(&self, index: usize) -> Option<f32> {
        self.runtime.parameter_value_by_index(index)
    }

    /// Writes a raw parameter value. Diagnostics and tests only; hosts set
    /// overrides through [`Model::set_parameters`].
    pub fn set_parameter_raw(&mut self, index: usize, value: f32) -> bool {
        self.runtime.set_parameter_by_index(index, value)
    }
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
