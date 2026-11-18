//! What the component tells the host.

use std::fmt;
use std::path::PathBuf;

use live2d_render::Error;

/// Something the component wants the host to know.
///
/// Delivered one frame late: the render layer cannot publish a message while it
/// is preparing a frame, so it hands events to the widget and the next `update`
/// publishes them.
#[derive(Debug)]
pub enum Live2dEvent {
    /// The model is loaded and is drawn from this frame on.
    ModelLoaded {
        /// What the model exposes: motions, groups, expressions.
        summary: ModelSummary,
    },
    /// The model could not be loaded, and will not be retried.
    ///
    /// The widget keeps reserving its space and draws nothing, so a host that
    /// wants the layout to collapse has to stop showing it itself.
    ModelFailedToLoad {
        /// Which path failed, and why.
        failure: LoadFailure,
    },
    /// Playback continues, but not the way the model file asked for.
    ///
    /// A cause that repeats every frame is reported once.
    ModelReportedProblem {
        /// What went wrong.
        problem: Problem,
    },
    /// A motion played to its end.
    ///
    /// Auto-played motions are reported too, so a host that cycles motions by
    /// index stays in step with the model.
    MotionFinishedPlaying {
        /// The motion that ended.
        motion: MotionId,
    },
}

/// What a model exposes, counted at load time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSummary {
    /// Motions reachable by index.
    pub motions: usize,
    /// Named motion groups, including the scanned fallback group.
    pub motion_groups: usize,
    /// Expressions reachable by index.
    pub expressions: usize,
}

/// Which model failed to load, and why.
#[derive(Debug)]
pub struct LoadFailure {
    /// The `.model3.json` that was asked for.
    pub path: PathBuf,
    /// The failure itself.
    pub error: Error,
}

/// A problem that did not stop playback.
#[derive(Debug)]
pub enum Problem {
    /// The render layer could not do something it was asked to do.
    Renderer {
        /// What went wrong.
        error: Error,
    },
    /// The host queued more than 64 commands before the render layer took them.
    ///
    /// The oldest were dropped, and `dropped` counts them.
    CommandQueueOverflow {
        /// How many commands were dropped.
        dropped: usize,
    },
}

impl fmt::Display for Problem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Renderer { error } => write!(formatter, "{error}"),
            Problem::CommandQueueOverflow { dropped } => write!(
                formatter,
                "dropped {dropped} command(s): the render layer is behind the host"
            ),
        }
    }
}

/// A motion by index, as the render layer knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MotionId {
    /// Index into the model's playable motions, in the order `motions` reports.
    pub index: usize,
}
