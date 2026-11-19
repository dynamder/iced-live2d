//! Cubism motion playback.
//!
//! Load a motion with [`crate::motion::load_motion`], wrap it in a
//! [`MotionPlayer`], and call [`MotionPlayer::tick`] plus [`MotionPlayer::apply`]
//! each frame before updating runtime meshes.

use std::{fs, path::Path};

use crate::{
    json::{
        Motion3, apply_motion_fade, easing_sine, motion_fade_in_weight, motion_fade_out_weight,
        parameter_curve_fade_weight,
    },
    runtime::ModelRuntime,
};

const PARAMETER_TARGET: &str = "Parameter";
const PART_OPACITY_TARGET: &str = "PartOpacity";

/// The fade Cubism uses when neither the motion file nor the model file declares one.
pub const DEFAULT_FADE_SECONDS: f32 = 1.0;

/// A forced fade-out in progress, counted down in whole ticks.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FadeOut {
    remaining: f32,
    total: f32,
}

#[derive(Debug, Clone)]
/// Plays a parsed `motion3.json` animation against a [`ModelRuntime`].
///
/// A player owns playback time and blend weight. It does not update meshes by
/// itself; call [`ModelRuntime::update_meshes`] after applying one or more
/// players.
pub struct MotionPlayer {
    motion: Motion3,
    time: f32,
    weight: f32,
    fade_in: f32,
    fade_out: f32,
    fade_out_now: Option<FadeOut>,
    looping: bool,
    finished: bool,
}

impl MotionPlayer {
    /// Creates a player at time `0.0` with full weight.
    ///
    /// The player follows the motion's own `Loop` metadata and fades over the times
    /// the motion file declares, or [`DEFAULT_FADE_SECONDS`] where it declares none.
    pub fn new(motion: Motion3) -> Self {
        let looping = motion.meta().is_looping();
        Self::with_looping(motion, looping)
    }

    /// Creates a player that stops at the end, ignoring the motion's `Loop` metadata.
    pub fn new_once(motion: Motion3) -> Self {
        Self::with_looping(motion, false)
    }

    /// Creates a player with an explicit loop mode.
    ///
    /// Fade times come from the motion file, or [`DEFAULT_FADE_SECONDS`] where it
    /// declares none.
    pub fn with_looping(motion: Motion3, looping: bool) -> Self {
        let fade_in = motion.meta().fade_in_time().unwrap_or(DEFAULT_FADE_SECONDS);
        let fade_out = motion
            .meta()
            .fade_out_time()
            .unwrap_or(DEFAULT_FADE_SECONDS);

        Self::with_looping_and_fade(motion, looping, fade_in, fade_out)
    }

    /// Creates a player with explicit fade times, in seconds.
    ///
    /// This is how a caller applies a value that wins over the motion's own: the
    /// render layer resolves the model file reference and its host setting before
    /// the player exists, so one place decides.
    pub fn with_fade(motion: Motion3, fade_in: f32, fade_out: f32) -> Self {
        let looping = motion.meta().is_looping();

        Self::with_looping_and_fade(motion, looping, fade_in, fade_out)
    }

    fn with_looping_and_fade(motion: Motion3, looping: bool, fade_in: f32, fade_out: f32) -> Self {
        Self {
            motion,
            time: 0.0,
            weight: 1.0,
            fade_in: fade_in.max(0.0),
            fade_out: fade_out.max(0.0),
            fade_out_now: None,
            looping,
            finished: false,
        }
    }

    /// Returns the motion data owned by this player.
    pub fn motion(&self) -> &Motion3 {
        &self.motion
    }

    /// Returns the current playback time in seconds.
    pub fn time(&self) -> f32 {
        self.time
    }

    /// Returns the player's global blend weight.
    pub fn weight(&self) -> f32 {
        self.weight
    }

    /// Returns whether this player wraps at the motion duration.
    pub fn is_looping(&self) -> bool {
        self.looping
    }

    /// Sets the player's global blend weight, clamped to `0.0..=1.0`.
    pub fn set_weight(&mut self, weight: f32) {
        self.weight = weight.clamp(0.0, 1.0);
    }

    /// Returns whether a one-shot motion has reached its end.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Restarts playback from the beginning, cancelling any fade-out in progress.
    pub fn restart(&mut self) {
        self.time = 0.0;
        self.fade_out_now = None;
        self.finished = false;
    }

    /// Starts fading this player out now, over its own fade-out time.
    ///
    /// A fade longer than the clip's remaining time ends with the clip, so the pose
    /// is never left half way. Returns whether anything is left to fade: `false`
    /// means the player is finished and can be dropped at once.
    pub fn fade_out(&mut self) -> bool {
        let remaining = if self.looping {
            f32::INFINITY
        } else {
            (self.motion.meta().duration() - self.time).max(0.0)
        };
        let total = self.fade_out.min(remaining);

        if total <= 0.0 {
            self.finished = true;
            return false;
        }

        self.fade_out_now = Some(FadeOut {
            remaining: total,
            total,
        });
        true
    }

    /// Advances playback time by `delta_seconds`.
    ///
    /// Negative deltas are treated as zero. Looping motions wrap at their
    /// declared duration; non-looping motions stop at the end.
    pub fn tick(&mut self, delta_seconds: f32) {
        if self.finished {
            return;
        }

        let delta = delta_seconds.max(0.0);

        if let Some(fade) = self.fade_out_now.as_mut() {
            fade.remaining = (fade.remaining - delta).max(0.0);

            if fade.remaining <= 0.0 {
                self.finished = true;
            }
        }

        self.time += delta;
        let duration = self.motion.meta().duration();
        if duration <= 0.0 {
            return;
        }

        if self.looping {
            self.time %= duration;
        } else if self.time >= duration {
            self.time = duration;
            self.finished = true;
        }
    }

    /// Applies the current motion sample to a model runtime.
    ///
    /// Curves targeting unknown parameters or parts are ignored. Call
    /// [`ModelRuntime::update_meshes`] after all motion and expression players
    /// have been applied for the frame.
    pub fn apply(&self, runtime: &mut ModelRuntime) {
        let duration = self.motion.meta().duration();
        let end_time = if self.looping { -1.0 } else { duration };
        let fade_in = motion_fade_in_weight(self.time, 0.0, self.fade_in);
        let fade_out = match self.fade_out_now {
            Some(fade) if fade.total > 0.0 => easing_sine(fade.remaining / fade.total),
            Some(_) => 0.0,
            None => motion_fade_out_weight(self.time, end_time, self.fade_out),
        };

        for curve in self.motion.curves() {
            let Some(sampled) = curve.sample(self.time) else {
                continue;
            };
            let curve_weight = parameter_curve_fade_weight(
                self.weight,
                fade_in,
                fade_out,
                curve.fade_in_time(),
                curve.fade_out_time(),
                self.time,
                0.0,
                end_time,
            );

            match curve.target() {
                PARAMETER_TARGET => {
                    let Some(index) = runtime.parameter_index(curve.id()) else {
                        continue;
                    };
                    let Some(current) = runtime.parameter_value_by_index(index) else {
                        continue;
                    };
                    let value = apply_motion_fade(current, sampled, curve_weight);
                    runtime.set_parameter_by_index(index, value);
                }
                PART_OPACITY_TARGET => {
                    let Some(index) = runtime.part_index(curve.id()) else {
                        continue;
                    };
                    let value = apply_motion_fade(1.0, sampled, curve_weight);
                    runtime.set_part_opacity_by_index(index, value);
                }
                _ => {}
            }
        }
    }
}

/// Loads a Cubism `motion3.json` file from disk.
pub fn load_motion(path: impl AsRef<Path>) -> Result<Motion3, MotionLoadError> {
    let path = path.as_ref();
    let source = fs::read_to_string(path).map_err(|source| MotionLoadError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Motion3::from_json_str(&source).map_err(MotionLoadError::Parse)
}

#[derive(Debug, thiserror::Error)]
/// Errors that can occur while loading a motion file.
pub enum MotionLoadError {
    /// The motion file could not be read.
    #[error("failed to read {path}: {source}")]
    Io {
        /// Path of the file that failed to load.
        path: String,
        /// Original I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The motion JSON was invalid or unsupported.
    #[error("failed to parse motion3: {0}")]
    Parse(#[source] crate::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURVES: &str = r#""Curves": [{"Target": "Parameter", "Id": "ParamAngleX", "Segments": [0, 0.0, 1, 1.0, 10.0]}]"#;

    fn motion(extra_meta: &str, duration: f32, looping: bool) -> Motion3 {
        let source = format!(
            r#"{{"Version": 3, "Meta": {{"Duration": {duration}, "Fps": 30.0, "Loop": {looping},
                "CurveCount": 1, "TotalSegmentCount": 1, "TotalPointCount": 2{extra_meta}}}, {CURVES}}}"#
        );

        Motion3::from_json_str(&source).expect("a valid motion")
    }

    #[test]
    fn without_a_declared_fade_a_player_uses_the_default() {
        let player = MotionPlayer::new(motion("", 1.0, false));

        assert_eq!(player.fade_in, DEFAULT_FADE_SECONDS);
        assert_eq!(player.fade_out, DEFAULT_FADE_SECONDS);
    }

    #[test]
    fn a_declared_fade_wins_over_the_default() {
        let player = MotionPlayer::new(motion(
            r#", "FadeInTime": 0.2, "FadeOutTime": 0.3"#,
            1.0,
            false,
        ));

        assert_eq!(player.fade_in, 0.2);
        assert_eq!(player.fade_out, 0.3);
    }

    #[test]
    fn a_declared_zero_is_a_value_not_a_missing_one() {
        let source = motion(r#", "FadeInTime": 0.0, "FadeOutTime": 0.0"#, 1.0, true);
        let player = MotionPlayer::new(source);

        assert_eq!(player.fade_in, 0.0);
        assert_eq!(player.fade_out, 0.0);
    }

    #[test]
    fn a_negative_fade_means_unset() {
        let source = motion(r#", "FadeInTime": -1.0"#, 1.0, false);

        assert_eq!(source.meta().fade_in_time(), None);
        assert_eq!(MotionPlayer::new(source).fade_in, DEFAULT_FADE_SECONDS);
    }

    #[test]
    fn an_explicit_fade_overrides_the_motion() {
        let player =
            MotionPlayer::with_fade(motion(r#", "FadeInTime": 0.2"#, 1.0, false), 0.0, 0.0);

        assert_eq!(player.fade_in, 0.0);
        assert_eq!(player.fade_out, 0.0);
    }

    #[test]
    fn a_zero_fade_out_finishes_the_player_at_once() {
        let mut player = MotionPlayer::with_fade(motion("", 1.0, false), 0.0, 0.0);

        assert!(!player.fade_out());
        assert!(player.is_finished());
    }

    #[test]
    fn fade_out_takes_the_declared_time() {
        let mut player = MotionPlayer::with_fade(motion("", 5.0, false), 0.0, 0.5);

        assert!(player.fade_out());

        player.tick(0.25);
        assert!(!player.is_finished());

        player.tick(0.3);
        assert!(player.is_finished());
    }

    #[test]
    fn fade_out_ends_with_a_short_clip() {
        let mut player = MotionPlayer::with_fade(motion("", 0.5, false), 0.0, 1.0);

        player.tick(0.4);
        assert!(player.fade_out());

        player.tick(0.05);
        assert!(!player.is_finished());

        player.tick(0.05);
        assert!(player.is_finished());
    }

    #[test]
    fn a_looping_player_fades_over_its_own_time() {
        let mut player = MotionPlayer::with_fade(motion("", 0.5, true), 0.0, 0.5);

        player.tick(0.6);
        assert!(player.fade_out());

        player.tick(0.5);
        assert!(player.is_finished());
    }
}
