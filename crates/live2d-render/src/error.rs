//! Errors surfaced by the render layer.

use std::path::PathBuf;

use mocari::assets::AssetLoadError;
use mocari::expression::ExpressionLoadError;
use mocari::motion::MotionLoadError;
use mocari::render::wgpu::{
    WgpuClippingLayoutError, WgpuMeshUpdateError, WgpuRenderError, WgpuTextureError,
};

/// The render layer's result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything the render layer can fail at.
///
/// [`Asset::parse`](crate::Asset::parse) and [`Prepared::upload`](crate::Prepared::upload)
/// report load failures; the rest are runtime problems worth surfacing without
/// stopping playback.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A model file, its `.moc3`, or one of its textures could not be read.
    #[error("failed to load Live2D model assets: {0}")]
    Assets(#[from] AssetLoadError),

    /// A motion file referenced by the model could not be read or parsed.
    #[error("failed to load motion {path}: {source}")]
    Motion {
        /// The motion file that failed.
        path: PathBuf,
        /// Why it failed.
        #[source]
        source: MotionLoadError,
    },

    /// An expression file referenced by the model could not be read or parsed.
    #[error("failed to load expression {path}: {source}")]
    Expression {
        /// The expression file that failed.
        path: PathBuf,
        /// Why it failed.
        #[source]
        source: ExpressionLoadError,
    },

    /// The model declares no canvas and no drawable vertices to frame it by.
    #[error("model has no drawable geometry")]
    NoDrawables,

    /// A motion group index is not one the model declares.
    #[error("motion group #{group} does not exist, the model exposes {count}")]
    MotionGroup {
        /// The group index that was asked for.
        group: usize,
        /// How many groups the model declares.
        count: usize,
    },

    /// A motion clip index is not one the group holds.
    #[error("motion clip #{clip} is out of range for group #{group}, which has {count}")]
    MotionClip {
        /// The group index that was asked for.
        group: usize,
        /// The clip index that was asked for.
        clip: usize,
        /// How many clips the group holds.
        count: usize,
    },

    /// An expression index is not one the model declares.
    #[error("expression #{index} is out of range, the model exposes {count}")]
    ExpressionIndex {
        /// The index that was asked for.
        index: usize,
        /// How many expressions the model declares.
        count: usize,
    },

    /// The model directory could not be scanned for sidecar animation files.
    #[error("failed to scan {path}: {source}")]
    Scan {
        /// The directory that failed.
        path: PathBuf,
        /// Why it failed.
        #[source]
        source: std::io::Error,
    },

    /// The `physics3.json` the model references could not be read.
    ///
    /// Physics is skipped and the look-at resolver falls back to the conventional
    /// parameter names; the model still draws.
    #[error("failed to read physics settings from {path}: {source}")]
    PhysicsFile {
        /// The file that failed.
        path: PathBuf,
        /// Why it failed.
        #[source]
        source: std::io::Error,
    },

    /// The `physics3.json` the model references could not be parsed.
    #[error("failed to parse physics settings in {path}")]
    PhysicsParse {
        /// The file that failed.
        path: PathBuf,
    },

    /// The drawable set could not be turned into GPU buffers.
    #[error("failed to allocate Live2D mesh buffers")]
    MeshBuffers,

    /// A host write was applied before physics to a parameter physics writes, so
    /// physics overwrites it in the same step.
    #[error(
        "parameter {name:?} is written by physics; a write before physics is overwritten (use ParameterStage::AfterPhysics)"
    )]
    ParameterOverwritten {
        /// The parameter whose write was overwritten.
        name: String,
    },

    /// The model data could not produce a valid mesh update.
    #[error("failed to rebuild Live2D meshes")]
    MeshUpdate,

    /// A drawable changed shape in a way the buffers cannot absorb, so the whole set
    /// was rebuilt. Worth reporting once: per-frame rebuilds look like a slow model.
    #[error("Live2D geometry changed shape, rebuilding every buffer: {0}")]
    MeshTopology(#[from] WgpuMeshUpdateError),

    /// A clipping layout could not be created. Clipped drawables cannot be drawn.
    #[error("failed to create a clipping layout: {0}")]
    ClippingLayout(#[from] WgpuClippingLayoutError),

    /// A texture could not be created on the GPU.
    #[error("failed to create a texture: {0}")]
    Texture(#[from] WgpuTextureError),

    /// A render pass could not be recorded.
    #[error("failed to draw Live2D geometry: {0}")]
    Render(#[from] WgpuRenderError),
}
