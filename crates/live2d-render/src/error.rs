//! Errors surfaced by the render layer.

use std::path::PathBuf;

use mocari::assets::AssetLoadError;
use mocari::expression::ExpressionLoadError;
use mocari::motion::MotionLoadError;
use mocari::render::wgpu::{
    WgpuClippingLayoutError, WgpuRenderError, WgpuTextureError,
};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to load Live2D model assets: {0}")]
    Assets(#[from] AssetLoadError),

    #[error("failed to load motion {path}: {source}")]
    Motion {
        path: PathBuf,
        #[source]
        source: MotionLoadError,
    },

    #[error("failed to load expression {path}: {source}")]
    Expression {
        path: PathBuf,
        #[source]
        source: ExpressionLoadError,
    },

    #[error("model has no drawable geometry")]
    NoDrawables,

    #[error("failed to allocate Live2D mesh buffers")]
    MeshBuffers,

    #[error("failed to rebuild Live2D meshes")]
    MeshUpdate,

    #[error("failed to create a clipping layout: {0}")]
    ClippingLayout(#[from] WgpuClippingLayoutError),

    #[error("failed to create a texture: {0}")]
    Texture(#[from] WgpuTextureError),

    #[error("failed to draw Live2D geometry: {0}")]
    Render(#[from] WgpuRenderError),
}
