//! What the component tells the host.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use live2d_render::{Error, ModelInfo};

use crate::interface::{Interface, MissingNames};

/// Something the component wants the host to know.
///
/// Delivered one frame late: the render layer cannot publish a message while it
/// is preparing a frame, so it hands events to the widget and the next `update`
/// publishes them.
///
/// Events are **per model**, not per widget: two widgets drawing one model both
/// receive all of them, and the second one to appear still receives the load
/// result.
pub enum Live2dEvent<I: Interface> {
    /// The model is loaded, validated against the interface, and drawn from this
    /// frame on.
    ///
    /// The `info` is the model's own description: every name, range and hit area
    /// it declares, and the asset identity that raw keys are minted with. It does
    /// not hand out typed keys.
    ModelLoaded {
        /// What the model exposes.
        info: Arc<ModelInfo>,
    },
    /// The model could not be loaded, or does not declare what the interface
    /// names, and will not be retried.
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
    /// A motion started playing.
    ///
    /// Includes the idle motions the model plays by itself, so a host that shows
    /// "now playing" stays in step with it.
    MotionStarted {
        /// The motion group the clip belongs to.
        group: I::Motion,
        /// Index of the clip inside that group.
        clip: usize,
        /// Length of the clip in seconds, as declared by the motion file.
        duration: f32,
        /// Whether the clip loops. A looping clip never reports
        /// [`Live2dEvent::MotionFinishedPlaying`].
        looping: bool,
    },
    /// A motion played to its end and was replaced by whatever the idle policy
    /// chose next.
    MotionFinishedPlaying {
        /// The motion group the clip belonged to.
        group: I::Motion,
        /// Index of the clip inside that group.
        clip: usize,
    },
    /// The pointer did something on the widget.
    Pointer(PointerEvent<I>),
}

/// A pointer interaction, in the terms a Live2D model speaks.
///
/// Every coordinate is in **model space** — the same space the model's own hit
/// areas are tested in — so it can be compared against
/// [`ModelInfo::bounds`] and fed back into hit testing without another
/// conversion. Use [`View`](live2d_render::View) semantics if you need clip space.
pub enum PointerEvent<I: Interface> {
    /// A button went down inside the widget.
    Pressed {
        /// The hit area under the cursor, if one was hit.
        area: Option<I::HitArea>,
        /// Where it happened, in model space.
        at: [f32; 2],
    },
    /// A button went up inside the widget.
    Released {
        /// The hit area under the cursor, if one was hit.
        area: Option<I::HitArea>,
        /// Where it happened, in model space.
        at: [f32; 2],
    },
    /// The cursor moved onto a hit area.
    Entered {
        /// The area the cursor is now over.
        area: I::HitArea,
    },
    /// The cursor left the hit area it was over.
    Exited {
        /// The area the cursor left.
        area: I::HitArea,
    },
    /// The cursor moved while a button was down.
    ///
    /// Only this is sent: whether a drag moves the window, turns the head or
    /// drives physics is the host's decision.
    Dragged {
        /// Movement since the previous frame, in model plane units.
        delta: [f32; 2],
    },
    /// The wheel turned inside the widget.
    ///
    /// Zooming, scrolling a list or nothing at all is the host's decision; pixel
    /// deltas are converted to lines by the widget.
    Scrolled {
        /// Vertical lines scrolled; positive is away from the user.
        lines: f32,
    },
}

impl<I: Interface> Clone for PointerEvent<I> {
    fn clone(&self) -> Self {
        match self {
            PointerEvent::Pressed { area, at } => PointerEvent::Pressed {
                area: area.clone(),
                at: *at,
            },
            PointerEvent::Released { area, at } => PointerEvent::Released {
                area: area.clone(),
                at: *at,
            },
            PointerEvent::Entered { area } => PointerEvent::Entered { area: area.clone() },
            PointerEvent::Exited { area } => PointerEvent::Exited { area: area.clone() },
            PointerEvent::Dragged { delta } => PointerEvent::Dragged { delta: *delta },
            PointerEvent::Scrolled { lines } => PointerEvent::Scrolled { lines: *lines },
        }
    }
}

impl<I: Interface> fmt::Debug for PointerEvent<I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PointerEvent::Pressed { area, at } => formatter
                .debug_struct("Pressed")
                .field("area", area)
                .field("at", at)
                .finish(),
            PointerEvent::Released { area, at } => formatter
                .debug_struct("Released")
                .field("area", area)
                .field("at", at)
                .finish(),
            PointerEvent::Entered { area } => {
                formatter.debug_struct("Entered").field("area", area).finish()
            }
            PointerEvent::Exited { area } => {
                formatter.debug_struct("Exited").field("area", area).finish()
            }
            PointerEvent::Dragged { delta } => formatter
                .debug_struct("Dragged")
                .field("delta", delta)
                .finish(),
            PointerEvent::Scrolled { lines } => formatter
                .debug_struct("Scrolled")
                .field("lines", lines)
                .finish(),
        }
    }
}

impl<I: Interface> Clone for Live2dEvent<I> {
    fn clone(&self) -> Self {
        match self {
            Live2dEvent::ModelLoaded { info } => Live2dEvent::ModelLoaded {
                info: Arc::clone(info),
            },
            Live2dEvent::ModelFailedToLoad { failure } => Live2dEvent::ModelFailedToLoad {
                failure: failure.clone(),
            },
            Live2dEvent::ModelReportedProblem { problem } => Live2dEvent::ModelReportedProblem {
                problem: problem.clone(),
            },
            Live2dEvent::MotionStarted {
                group,
                clip,
                duration,
                looping,
            } => Live2dEvent::MotionStarted {
                group: group.clone(),
                clip: *clip,
                duration: *duration,
                looping: *looping,
            },
            Live2dEvent::MotionFinishedPlaying { group, clip } => {
                Live2dEvent::MotionFinishedPlaying {
                    group: group.clone(),
                    clip: *clip,
                }
            }
            Live2dEvent::Pointer(event) => Live2dEvent::Pointer(event.clone()),
        }
    }
}

impl<I: Interface> fmt::Debug for Live2dEvent<I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Live2dEvent::ModelLoaded { info } => formatter
                .debug_struct("ModelLoaded")
                .field("info", info)
                .finish(),
            Live2dEvent::ModelFailedToLoad { failure } => formatter
                .debug_struct("ModelFailedToLoad")
                .field("failure", failure)
                .finish(),
            Live2dEvent::ModelReportedProblem { problem } => formatter
                .debug_struct("ModelReportedProblem")
                .field("problem", problem)
                .finish(),
            Live2dEvent::MotionStarted {
                group,
                clip,
                duration,
                looping,
            } => formatter
                .debug_struct("MotionStarted")
                .field("group", group)
                .field("clip", clip)
                .field("duration", duration)
                .field("looping", looping)
                .finish(),
            Live2dEvent::MotionFinishedPlaying { group, clip } => formatter
                .debug_struct("MotionFinishedPlaying")
                .field("group", group)
                .field("clip", clip)
                .finish(),
            Live2dEvent::Pointer(event) => formatter.debug_tuple("Pointer").field(event).finish(),
        }
    }
}

/// Which model failed to load, and why.
#[derive(Debug, Clone)]
pub enum LoadFailure {
    /// The model could not be read, parsed, or uploaded.
    Renderer {
        /// The `.model3.json` that was asked for.
        path: PathBuf,
        /// The failure itself.
        ///
        /// Shared so events stay cloneable, which hosts commonly want from their
        /// message type.
        error: Arc<Error>,
    },
    /// The model loaded, but does not declare everything the interface names.
    ///
    /// Nothing was uploaded for it: validation happens before the textures go to
    /// the GPU, so a mismatched interface costs no VRAM.
    Interface {
        /// The `.model3.json` that was asked for.
        path: PathBuf,
        /// Which names it lacks.
        missing: MissingNames,
    },
}

impl fmt::Display for LoadFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadFailure::Renderer { path, error } => {
                write!(formatter, "{}: {error}", path.display())
            }
            LoadFailure::Interface { path, missing } => {
                write!(formatter, "{}: interface mismatch: {missing}", path.display())
            }
        }
    }
}

/// A problem that did not stop playback.
#[derive(Debug, Clone)]
pub enum Problem {
    /// The render layer could not do something it was asked to do.
    Renderer {
        /// What went wrong. Shared so events stay cloneable.
        error: Arc<Error>,
    },
    /// The host queued more than 64 commands before the render layer took them.
    ///
    /// The oldest were dropped, and `dropped` counts them.
    CommandQueueOverflow {
        /// How many commands were dropped.
        dropped: usize,
    },
    /// A key minted from one asset was handed to a model of another.
    ///
    /// Raw keys carry the asset they came from, so this is caught instead of
    /// silently landing on the same index. The key was rejected; it is reported
    /// once per distinct name.
    ForeignKey {
        /// Which vocabulary the key belongs to: `"parameter"` or `"part"`.
        vocabulary: &'static str,
        /// The key's name in its own model.
        name: String,
    },
    /// A key does not exist in the vocabulary that produced it.
    ///
    /// Only a hand-written [`Vocabulary`](crate::Vocabulary) whose `index` and
    /// `from_id` disagree with its `SPEC` can cause this; the event that carried
    /// it was dropped.
    Vocabulary {
        /// Which vocabulary the entry belongs to, such as `"motion"`.
        vocabulary: &'static str,
        /// The index the render layer reported.
        index: usize,
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
            Problem::ForeignKey { vocabulary, name } => write!(
                formatter,
                "ignored {vocabulary} key {name:?}: it belongs to a different asset"
            ),
            Problem::Vocabulary { vocabulary, index } => write!(
                formatter,
                "ignored {vocabulary} #{index}: the vocabulary does not cover it"
            ),
        }
    }
}
