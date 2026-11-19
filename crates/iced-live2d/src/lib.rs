//! Live2D models as an iced widget.
//!
//! The widget owns no state a host has to thread through its update loop: build a
//! [`Live2dAsset`], make one or more [`Live2dModel`]s from it, hand a reference to
//! [`live2d`], and react to [`Live2dEvent`]s.
//!
//! The pieces:
//!
//! - [`Live2dAsset`] — one model file. Loaded on the first frame that draws a
//!   model of it, shared by every model, immutable afterwards.
//! - [`Live2dModel`] — one instance: its own parameters, playback and physics.
//! - [`Interface`] — the vocabulary the host drives the model through, or
//!   [`raw::Raw`] for "whatever this model declares".
//! - [`AssetOptions`] / [`ModelOptions`] — the texture and mask budgets.
//! - [`look_at::cursor`] — turns the cursor into a LookAt target.
//! - [`Live2dEvent`] — load outcome, problems, motion transitions, pointer input.
//!
//! ```text
//! let asset = Live2dAsset::from_path("Hiyori.model3.json");
//! let model = Live2dModel::<Face>::new(&asset);
//! // ... in the host's view:
//! live2d(&model)
//!     .on_event(Message::Live2d)
//!     .look_at(look_at::cursor)
//! ```
//!
//! Only the wgpu renderer works: the widget delegates to iced's `shader` widget,
//! which the tiny-skia backend does not have.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod event;
mod interface;
mod model;
mod pipeline;
mod widget;

pub mod look_at;
pub mod raw;

pub use event::{Live2dEvent, LoadFailure, PointerEvent, Problem};
pub use interface::{Interface, InterfaceSpec, KeyId, MissingNames, Vocabulary, VocabularySpec};
pub use look_at::LookAtSource;
pub use model::{Idle, Live2dAsset, Live2dModel, Parameters, Parts};
pub use raw::{Raw, RawExpression, RawHitArea, RawKey, RawMotion, RawParameter, RawPart};
pub use widget::{Live2d, Live2dState, live2d};

/// Derives [`Vocabulary`] for a fieldless enum.
///
/// Behind the `macros` feature, which is off by default: the component itself
/// never needs the derive, and a host that prefers to write the four items by hand
/// pays nothing for it. See [`Vocabulary`] for what is generated.
#[cfg(feature = "macros")]
pub use iced_live2d_macros::Vocabulary;

pub use live2d_render::{
    AssetId, AssetOptions, Error, HitArea, LookAtMode, ModelBounds, ModelInfo, ModelOptions,
    MotionGroup, ParameterRange,
};

/// Compile-time check that events may cross the render-to-UI channel.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Live2dEvent<raw::Raw>>();
    assert_send::<Problem>();
};
