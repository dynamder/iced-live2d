//! What the component tells the host.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use live2d_render::{Error, ModelInfo};

use crate::interface::{Interface, MissingNames};
use crate::model::Live2dId;

/// Something the component wants the host to know, delivered one frame late.
///
/// Events are **per model**, not per widget: two widgets drawing one model both
/// receive all of them, and the second still receives the load result.
pub enum Live2dEvent<I: Interface> {
    /// The model is loaded, its interface validated, and drawn from this frame on.
    ///
    /// `info` is the asset's own description, and the identity raw keys are minted
    /// with.
    ModelLoaded {
        /// Which model.
        model: Live2dId,
        /// What the model exposes.
        info: Arc<ModelInfo>,
    },
    /// The model could not be loaded, or lacks what the interface names. It is not
    /// retried; the widget keeps reserving its space, so a host that wants the
    /// layout to collapse has to stop showing it.
    ModelFailedToLoad {
        /// Which model.
        model: Live2dId,
        /// Which path failed, and why.
        failure: LoadFailure,
    },
    /// Playback continues, but not the way the model file asked for. A cause that
    /// repeats every frame is reported once.
    ModelReportedProblem {
        /// Which model.
        model: Live2dId,
        /// What went wrong.
        problem: Problem,
    },
    /// A motion started playing.
    MotionStarted {
        /// Which model.
        model: Live2dId,
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
    /// A non-looping motion played to its end.
    MotionFinishedPlaying {
        /// Which model.
        model: Live2dId,
        /// The motion group the clip belonged to.
        group: I::Motion,
        /// Index of the clip inside that group.
        clip: usize,
    },
    /// The pointer did something on the widget.
    Pointer {
        /// Which model.
        model: Live2dId,
        /// What the pointer did.
        event: PointerEvent<I>,
    },
}

impl<I: Interface> Live2dEvent<I> {
    /// Which model this is about.
    pub fn model(&self) -> Live2dId {
        match self {
            Live2dEvent::ModelLoaded { model, .. }
            | Live2dEvent::ModelFailedToLoad { model, .. }
            | Live2dEvent::ModelReportedProblem { model, .. }
            | Live2dEvent::MotionStarted { model, .. }
            | Live2dEvent::MotionFinishedPlaying { model, .. }
            | Live2dEvent::Pointer { model, .. } => *model,
        }
    }
}

/// A pointer interaction, in the terms a Live2D model speaks.
///
/// Coordinates are in **model space**, the space hit areas are tested in, so they
/// compare against [`ModelInfo::bounds`] directly.
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
    /// The cursor moved while a button was down. What that does — move the window,
    /// turn the head — is the host's decision.
    Dragged {
        /// Movement since the previous frame, in model plane units.
        delta: [f32; 2],
    },
    /// The wheel turned inside the widget. The widget converts pixel deltas to
    /// lines; zooming or scrolling is the host's decision.
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
            PointerEvent::Entered { area } => formatter
                .debug_struct("Entered")
                .field("area", area)
                .finish(),
            PointerEvent::Exited { area } => formatter
                .debug_struct("Exited")
                .field("area", area)
                .finish(),
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
            Live2dEvent::ModelLoaded { model, info } => Live2dEvent::ModelLoaded {
                model: *model,
                info: Arc::clone(info),
            },
            Live2dEvent::ModelFailedToLoad { model, failure } => Live2dEvent::ModelFailedToLoad {
                model: *model,
                failure: failure.clone(),
            },
            Live2dEvent::ModelReportedProblem { model, problem } => {
                Live2dEvent::ModelReportedProblem {
                    model: *model,
                    problem: problem.clone(),
                }
            }
            Live2dEvent::MotionStarted {
                model,
                group,
                clip,
                duration,
                looping,
            } => Live2dEvent::MotionStarted {
                model: *model,
                group: group.clone(),
                clip: *clip,
                duration: *duration,
                looping: *looping,
            },
            Live2dEvent::MotionFinishedPlaying { model, group, clip } => {
                Live2dEvent::MotionFinishedPlaying {
                    model: *model,
                    group: group.clone(),
                    clip: *clip,
                }
            }
            Live2dEvent::Pointer { model, event } => Live2dEvent::Pointer {
                model: *model,
                event: event.clone(),
            },
        }
    }
}

impl<I: Interface> fmt::Debug for Live2dEvent<I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Live2dEvent::ModelLoaded { model, info } => formatter
                .debug_struct("ModelLoaded")
                .field("model", model)
                .field("info", info)
                .finish(),
            Live2dEvent::ModelFailedToLoad { model, failure } => formatter
                .debug_struct("ModelFailedToLoad")
                .field("model", model)
                .field("failure", failure)
                .finish(),
            Live2dEvent::ModelReportedProblem { model, problem } => formatter
                .debug_struct("ModelReportedProblem")
                .field("model", model)
                .field("problem", problem)
                .finish(),
            Live2dEvent::MotionStarted {
                model,
                group,
                clip,
                duration,
                looping,
            } => formatter
                .debug_struct("MotionStarted")
                .field("model", model)
                .field("group", group)
                .field("clip", clip)
                .field("duration", duration)
                .field("looping", looping)
                .finish(),
            Live2dEvent::MotionFinishedPlaying { model, group, clip } => formatter
                .debug_struct("MotionFinishedPlaying")
                .field("model", model)
                .field("group", group)
                .field("clip", clip)
                .finish(),
            Live2dEvent::Pointer { model, event } => formatter
                .debug_struct("Pointer")
                .field("model", model)
                .field("event", event)
                .finish(),
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
    /// Validation happens before any upload, so this costs no VRAM.
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
                write!(
                    formatter,
                    "{}: interface mismatch: {missing}",
                    path.display()
                )
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
    /// More than 64 commands were queued before the render layer took them. The
    /// oldest were dropped.
    CommandQueueOverflow {
        /// How many commands were dropped.
        dropped: usize,
    },
    /// A key minted from one asset was handed to a model of another. It was
    /// rejected, and is reported once per distinct name.
    ForeignKey {
        /// Which vocabulary the key belongs to: `"parameter"` or `"part"`.
        vocabulary: &'static str,
        /// The key's name in its own model.
        name: String,
    },
    /// A key does not exist in the vocabulary that produced it: only a hand-written
    /// [`Vocabulary`](crate::Vocabulary) can cause this. The event was dropped.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface::{KeyId, Vocabulary};
    use crate::raw::{Raw, RawMotion};

    fn motion(asset: live2d_render::AssetId) -> RawMotion {
        RawMotion::from_id(&KeyId {
            asset,
            index: 0,
            name: Arc::from("Idle"),
        })
        .expect("a raw key")
    }

    #[test]
    fn every_event_reports_its_model() {
        let model = Live2dId::unique();
        let asset = live2d_render::AssetId::unique();

        let pointer = Live2dEvent::<Raw>::Pointer {
            model,
            event: PointerEvent::Dragged { delta: [1.0, 2.0] },
        };
        let started = Live2dEvent::<Raw>::MotionStarted {
            model,
            group: motion(asset),
            clip: 0,
            duration: 1.0,
            looping: false,
        };

        assert_eq!(pointer.model(), model);
        assert_eq!(started.model(), model);
    }
}
