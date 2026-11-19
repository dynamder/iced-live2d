//! Engine-agnostic Live2D playback + rendering layer.
//!
//! This crate deliberately depends only on `wgpu` and `mocari`, never on iced.
//! Everything iced-specific lives in `crates/iced-live2d`, so a wgpu/iced upgrade
//! touches exactly one layer.
//!
//! The types map one-to-one onto what a host has to juggle:
//!
//! - [`Asset`] — one `.model3.json`, its textures and everything it declares.
//!   Immutable once loaded, and shared by any number of instances.
//! - [`Model`] — one instance: parameters, playback, physics. Pure CPU, so it can
//!   be stepped and tested without a device.
//! - [`ModelGpu`] — the instance's GPU mirror.
//! - [`View`] — how one widget frames the model, and the matrix that maps a
//!   cursor back to model space.
//!
//! ```text
//! let prepared = Asset::parse(AssetId::unique(), "Hiyori.model3.json")?;
//! let asset = prepared.upload(&renderer, &device, &queue, AssetOptions::default())?;
//! let mut model = asset.model();
//! model.play_motion(0, 0);
//! if model.advance(STEP) > 0 && model.deform() {
//!     gpu.upload(&renderer, &device, &queue, &model)?;
//! }
//! ```

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod asset;
mod bounds;
mod error;
mod gpu;
mod info;
mod model;
mod rig;
mod view;

pub use asset::{
    Asset, AssetOptions, IDLE_GROUP, ModelOptions, Prepared, SCANNED_GROUP, declared_idle,
};
pub use bounds::ModelBounds;
pub use error::{Error, Result};
pub use gpu::ModelGpu;
pub use info::{AssetId, HitArea, ModelInfo, MotionGroup, ParameterRange};
pub use model::{Finished, Model, STEP, Started};
pub use rig::{Axis, Binding, LookAtMode, ModelFacts, Rig, Route, Slot, Source, resolve};
pub use view::{ClipRect, View};

pub use mocari;
pub use wgpu;

/// The pipelines every asset and instance draws through.
///
/// One of these is enough for any number of assets in one render format; keep it
/// next to them.
pub use mocari::render::wgpu::WgpuLive2dRenderer;
