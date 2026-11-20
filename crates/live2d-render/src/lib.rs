//! Engine-agnostic Live2D playback and rendering. Depends on `wgpu` and `mocari`,
//! never on iced.
//!
//! [`Asset`] is one `.model3.json` with its textures, shared by any number of
//! [`Model`] instances. A [`Model`] is pure CPU — parameters, playback, physics —
//! and [`ModelGpu`] is its GPU mirror; [`View`] frames it. The per-frame order is
//! `advance` → `deform` → `upload`:
//!
//! ```text
//! let prepared = Asset::parse(AssetId::unique(), "Hiyori.model3.json", options)?;
//! let asset = prepared.upload(&renderer, &device, &queue, options)?;
//! let mut model = asset.model();
//! model.play_motion(MotionGroupIndex::new(0), MotionClipIndex::new(0));
//! if model.advance(STEP) > 0 && model.deform() {
//!     gpu.upload(&renderer, &device, &queue, &model)?;
//! }
//! ```

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod asset;
mod bounds;
mod error;
mod fade;
mod gpu;
mod index;
mod info;
mod model;
mod policy;
mod rig;
mod view;

pub use asset::{Asset, AssetOptions, IDLE_GROUP, ModelOptions, Prepared, SCANNED_GROUP};
pub use bounds::{InvalidBounds, ModelBounds};
pub use error::{Error, Result};
pub use fade::Fade;
pub use gpu::ModelGpu;
pub use index::{
    ExpressionIndex, HitAreaIndex, MotionClipIndex, MotionGroupIndex, ParameterIndex, PartIndex,
};
pub use info::{AssetId, HitArea, ModelInfo, MotionGroup, ParameterRange};
pub use model::{Ended, Model, MotionEnd, STEP, Started};
pub use policy::{LookAtBlend, ParameterBlend, ParameterPolicy, ParameterStage, ParameterWrite};
pub use rig::{Axis, Binding, LookAtMode, ModelFacts, Rig, Route, Slot, Source, resolve};
pub use view::{ClipRect, View};

pub use mocari;
pub use wgpu;

/// The pipelines every asset and instance draws through. One is enough for any
/// number of assets in one render format.
pub use mocari::render::wgpu::WgpuLive2dRenderer;
