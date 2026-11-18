//! Engine-agnostic Live2D playback + rendering layer.
//!
//! This crate deliberately depends only on `wgpu` and `mocari`, never on iced.
//! Everything iced-specific lives in the `app` crate, so a wgpu/iced upgrade
//! touches exactly one layer.

mod bounds;
mod error;
mod rig;
mod scene;

pub use bounds::ModelBounds;
pub use error::{Error, Result};
pub use rig::{Binding, ModelFacts, Rig, Route, Slot, Source, TrackingMode, resolve};
pub use scene::{ClipRect, Live2dScene, SceneOptions};

pub use mocari;
pub use wgpu;
