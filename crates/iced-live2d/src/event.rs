//! What the component tells the host.
//!
//! Model events and interaction events are different types on purpose. What the
//! *model* says — loaded, failed, problems, motions — is the same whatever
//! vocabulary the widget reports, so [`ModelEvent`] carries no input parameter.
//! What the *pointer* did depends on that vocabulary: [`RawInput`] is a stream of
//! pointer facts, [`GestureInput`] a stream of classified gestures.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use live2d_render::{Error, ModelInfo, MotionClipIndex, MotionEnd};

use crate::interaction::{Gesture, PointerEvent};
use crate::interface::{Interface, MissingNames};
use crate::model::Live2dId;

/// Something one model did, delivered one frame late.
///
/// Events are **per model**, not per widget: two widgets drawing one model both
/// receive all of them, and the second still receives the load result.
pub enum ModelEvent<I: Interface> {
    /// The model is loaded, its interface validated, and drawn from this frame on.
    ///
    /// `info` is the asset's own description, and the identity raw keys are minted
    /// with.
    Loaded {
        /// Which model.
        model: Live2dId,
        /// What the model exposes.
        info: Arc<ModelInfo>,
    },
    /// The model could not be loaded, or lacks what the interface names. It is not
    /// retried; the widget keeps reserving its space, so a host that wants the
    /// layout to collapse has to stop showing it.
    FailedToLoad {
        /// Which model.
        model: Live2dId,
        /// Which path failed, and why.
        failure: LoadFailure,
    },
    /// Playback continues, but not the way the model file asked for. A cause that
    /// repeats every frame is reported once.
    Problem {
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
}

impl<I: Interface> ModelEvent<I> {
    /// Which model this is about.
    pub fn model(&self) -> Live2dId {
        match self {
            ModelEvent::Loaded { model, .. }
            | ModelEvent::FailedToLoad { model, .. }
            | ModelEvent::Problem { model, .. }
            | ModelEvent::MotionStarted { model, .. }
            | ModelEvent::MotionEnded { model, .. } => *model,
        }
    }
}

impl<I: Interface> Clone for ModelEvent<I> {
    fn clone(&self) -> Self {
        match self {
            ModelEvent::Loaded { model, info } => ModelEvent::Loaded {
                model: *model,
                info: Arc::clone(info),
            },
            ModelEvent::FailedToLoad { model, failure } => ModelEvent::FailedToLoad {
                model: *model,
                failure: failure.clone(),
            },
            ModelEvent::Problem { model, problem } => ModelEvent::Problem {
                model: *model,
                problem: problem.clone(),
            },
            ModelEvent::MotionStarted {
                model,
                group,
                clip,
                duration,
                looping,
            } => ModelEvent::MotionStarted {
                model: *model,
                group: group.clone(),
                clip: *clip,
                duration: *duration,
                looping: *looping,
            },
            ModelEvent::MotionEnded {
                model,
                group,
                clip,
                ended,
            } => ModelEvent::MotionEnded {
                model: *model,
                group: group.clone(),
                clip: *clip,
                ended: *ended,
            },
        }
    }
}

impl<I: Interface> fmt::Debug for ModelEvent<I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelEvent::Loaded { model, info } => formatter
                .debug_struct("Loaded")
                .field("model", model)
                .field("info", info)
                .finish(),
            ModelEvent::FailedToLoad { model, failure } => formatter
                .debug_struct("FailedToLoad")
                .field("model", model)
                .field("failure", failure)
                .finish(),
            ModelEvent::Problem { model, problem } => formatter
                .debug_struct("Problem")
                .field("model", model)
                .field("problem", problem)
                .finish(),
            ModelEvent::MotionStarted {
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
            ModelEvent::MotionEnded {
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
        }
    }
}

/// Everything a widget reports as raw input.
pub type RawInput<I> = PointerEvent<<I as Interface>::HitArea>;

/// Everything a widget reports as classified input.
pub type GestureInput<I> = Gesture<<I as Interface>::HitArea>;

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
    /// The asset loaded, but something in it did not: a sidecar it could not read,
    /// a physics file it could not parse. Drawing continues.
    Asset {
        /// What was skipped. Shared so events stay cloneable.
        error: Arc<Error>,
    },
    /// More than 64 commands were queued before the render layer took them. The
    /// oldest were dropped.
    CommandQueueOverflow {
        /// How many commands were dropped.
        dropped: usize,
    },
    /// More than 64 interaction events were queued before the host took them. The
    /// oldest were dropped, keeping the latest position of each pointer.
    EventQueueOverflow {
        /// How many events were dropped.
        dropped: usize,
    },
    /// A key minted from one asset was handed to a model of another. It was
    /// rejected, and is reported once per distinct name.
    ForeignKey {
        /// Which vocabulary the key belongs to: `"parameter"`, `"part"`,
        /// `"motion"` or `"expression"`.
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
            Problem::Renderer { error } | Problem::Asset { error } => write!(formatter, "{error}"),
            Problem::CommandQueueOverflow { dropped } => write!(
                formatter,
                "dropped {dropped} command(s): the render layer is behind the host"
            ),
            Problem::EventQueueOverflow { dropped } => write!(
                formatter,
                "dropped {dropped} event(s): the host is behind the widget"
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

        let pointer = RawInput::<Raw>::Moved(Moving {
            model,
            at: Sample {
                pixels: iced::Point::ORIGIN,
                clip: [0.0, 0.0],
                model: [0.0, 0.0],
            },
            when: Instant::now(),
        });
        let started = ModelEvent::<Raw>::MotionStarted {
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
