//! Live2D models as an iced widget.
//!
//! The widget owns no state a host has to thread through its update loop: build a
//! [`Live2dModel`], keep it in the host's state, hand a reference to
//! [`live2d`], and react to [`Live2dEvent`]s.
//!
//! The pieces:
//!
//! - [`Live2dModel`] — one model, loaded on the first frame that draws it, with
//!   its own GPU state and an RAII lifetime.
//! - [`RenderSettings`] — the GPU budget per model.
//! - [`look_at::cursor`] — turns the cursor into a LookAt target.
//! - [`Live2dEvent`] — load outcome, problems, finished motions.
//!
//! Only the wgpu renderer works: the widget delegates to iced's `shader` widget,
//! which the tiny-skia backend does not have.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod event;
mod model;
mod pipeline;
mod widget;

pub mod look_at;

pub use event::{Live2dEvent, LoadFailure, ModelSummary, MotionId, Problem};
pub use live2d_render::{Error, LookAtMode};
pub use look_at::LookAtSource;
pub use model::{Live2dModel, Live2dModelBuilder, NoSettings, RenderSettings, WithSettings};
pub use widget::{Live2d, live2d};

/// Compile-time check that events may cross the render-to-UI channel.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Live2dEvent>();
};
