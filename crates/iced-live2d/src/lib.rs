//! Live2D models as an iced widget.
//!
//! One [`Live2dAsset`] per file, one [`Live2dModel`] per instance, and a
//! [`live2d`] widget that borrows the model. The host decides what plays and reacts
//! to [`Live2dEvent`]s; the component has no policy of its own.
//!
//! ```text
//! let asset = Live2dAsset::new("Hiyori.model3.json", AssetOptions::default());
//! let model = Live2dModel::<Face>::new(&asset, ModelOptions::default());
//! live2d(&model).on_event(Message::Live2d).look_at(look_at::cursor)
//! ```
//!
//! [`Interface`] is the vocabulary the host drives the model through, or
//! [`Raw`] for whatever the model declares. wgpu only.

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
pub use model::{Live2dAsset, Live2dId, Live2dModel, Parameters, Parts};
pub use raw::{Raw, RawExpression, RawHitArea, RawKey, RawMotion, RawParameter, RawPart};
pub use widget::{Live2d, Live2dState, live2d};

/// Derives [`Vocabulary`] for a fieldless enum. Behind the `macros` feature, off by
/// default. See [`Vocabulary`] for what is generated.
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
