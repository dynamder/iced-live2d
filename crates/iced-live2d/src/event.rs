//! What the component tells the host.
//!
//! Two kinds of thing arrive here, and they are different types on purpose: what the
//! *model* says (loaded, failed, problems, motions) and what the *pointer* did. The
//! second one depends on the widget's vocabulary — raw facts or classified gestures —
//! so a widget reports `Live2dEvent<I, PointerEvent<I::HitArea>>` or
//! `Live2dEvent<I, Gesture<I::HitArea>>` and never both.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use live2d_render::{Error, ModelInfo, MotionClipIndex, MotionEnd};

use crate::interaction::{Gesture, PointerEvent};
use crate::interface::{Interface, MissingNames};
use crate::model::Live2dId;

/// Something the component wants the host to know, delivered one frame late.
///
/// Events are **per model**, not per widget: two widgets drawing one model both
/// receive all of them, and the second still receives the load result.
pub enum Live2dEvent<I: Interface, In> {
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
        /// The clip inside that group.
        clip: MotionClipIndex,
        /// Length of the clip in seconds, as declared by the motion file.
        duration: f32,
        /// Whether the clip loops. A looping clip never completes on its own, so it
        /// only ever ends as [`MotionEnd::Interrupted`].
        looping: bool,
    },
    /// The motion that was playing stopped, whichever way it ended.
    ///
    /// Every motion that starts reports here exactly once, so a host tracks playback
    /// from this event alone — including the clip that was replaced mid-way.
    MotionEnded {
        /// Which model.
        model: Live2dId,
        /// The motion group the clip belonged to.
        group: I::Motion,
        /// The clip inside that group.
        clip: MotionClipIndex,
        /// Whether it reached its own end or was stopped.
        ended: MotionEnd,
    },
    /// The pointer did something, in the widget's vocabulary.
    Input(In),
}

/// Everything one raw widget reports: the model's own events and raw pointer events.
pub type RawEvent<I> = Live2dEvent<I, PointerEvent<<I as Interface>::HitArea>>;

/// Everything one classified widget reports: the model's own events and gestures.
pub type GestureEvent<I> = Live2dEvent<I, Gesture<<I as Interface>::HitArea>>;

impl<I: Interface, In> Live2dEvent<I, In> {
    /// Which model this is about.
    pub fn model(&self) -> Live2dId
    where
        In: Associated,
    {
        match self {
            Live2dEvent::ModelLoaded { model, .. }
            | Live2dEvent::ModelFailedToLoad { model, .. }
            | Live2dEvent::ModelReportedProblem { model, .. }
            | Live2dEvent::MotionStarted { model, .. }
            | Live2dEvent::MotionEnded { model, .. } => *model,
            Live2dEvent::Input(input) => input.model(),
        }
    }
}

/// Anything an input payload can be asked of.
pub trait Associated {
    /// Which model this happened on.
    fn model(&self) -> Live2dId;
}

impl<A> Associated for PointerEvent<A> {
    fn model(&self) -> Live2dId {
        PointerEvent::model(self)
    }
}

impl<A> Associated for Gesture<A> {
    fn model(&self) -> Live2dId {
        Gesture::model(self)
    }
}

impl<I: Interface, In: Clone> Clone for Live2dEvent<I, In> {
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
            Live2dEvent::MotionEnded {
                model,
                group,
                clip,
                ended,
            } => Live2dEvent::MotionEnded {
                model: *model,
                group: group.clone(),
                clip: *clip,
                ended: *ended,
            },
            Live2dEvent::Input(input) => Live2dEvent::Input(input.clone()),
        }
    }
}

impl<I: Interface, In: fmt::Debug> fmt::Debug for Live2dEvent<I, In> {
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
            Live2dEvent::MotionEnded {
                model,
                group,
                clip,
                ended,
            } => formatter
                .debug_struct("MotionEnded")
                .field("model", model)
                .field("group", group)
                .field("clip", clip)
                .field("ended", ended)
                .finish(),
            Live2dEvent::Input(input) => formatter.debug_tuple("Input").field(input).finish(),
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
    use crate::interaction::{Moving, Sample};
    use crate::interface::{KeyId, Vocabulary};
    use crate::raw::{Raw, RawMotion};
    use std::time::Instant;

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

        let pointer = RawEvent::<Raw>::Input(PointerEvent::Moved(Moving {
            model,
            at: Sample {
                pixels: iced::Point::ORIGIN,
                clip: [0.0, 0.0],
                model: [0.0, 0.0],
            },
            when: Instant::now(),
        }));
        let started = RawEvent::<Raw>::MotionStarted {
            model,
            group: motion(asset),
            clip: MotionClipIndex::new(0),
            duration: 1.0,
            looping: false,
        };

        assert_eq!(pointer.model(), model);
        assert_eq!(started.model(), model);
    }
}
