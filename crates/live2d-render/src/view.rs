//! How one widget frames the model it draws.

use mocari::core::Matrix44;
use mocari::render::wgpu::{WgpuLive2dRenderer, WgpuTransform};
use wgpu::{Device, Queue};

use crate::bounds::ModelBounds;

/// The clip-space range a [`View`] with `fill == 1.0` spans.
const NDC: f32 = 2.0;

/// Below this a framing change is not worth a uniform upload.
const TOLERANCE: f32 = 1e-4;

/// A scissor/viewport rectangle in physical target pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipRect {
    /// Left edge, in physical pixels.
    pub x: u32,
    /// Top edge, in physical pixels.
    pub y: u32,
    /// Width, in physical pixels.
    pub width: u32,
    /// Height, in physical pixels.
    pub height: u32,
}

impl ClipRect {
    /// Whether the rectangle covers no pixels.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// The framing of one model in one widget: bounds, aspect and fill.
///
/// The same matrix goes to the GPU and is inverted for hit testing, so a click
/// cannot disagree with the pixels under it.
#[derive(Debug)]
pub struct View {
    bounds: ModelBounds,
    matrix: Matrix44,
    transform: WgpuTransform,
    aspect: f32,
    fill: f32,
}

impl View {
    /// Frames `bounds` in a widget of the given `aspect` (width / height), with the
    /// model spanning `fill` of it (`1.0` touches the edges).
    ///
    /// A non-positive or non-finite value of either falls back to `1.0`.
    pub fn new(
        renderer: &WgpuLive2dRenderer,
        device: &Device,
        bounds: ModelBounds,
        aspect: f32,
        fill: f32,
    ) -> Self {
        let aspect = sane_aspect(aspect);
        let fill = sane_fill(fill);
        let matrix = bounds.fit_matrix(aspect, fill * NDC);
        let transform = renderer.create_transform(device, &matrix);

        Self {
            bounds,
            matrix,
            transform,
            aspect,
            fill,
        }
    }

    /// Updates the aspect ratio; returns whether the framing changed.
    pub fn set_aspect(&mut self, queue: &Queue, aspect: f32) -> bool {
        let aspect = sane_aspect(aspect);

        if (self.aspect - aspect).abs() < TOLERANCE {
            return false;
        }

        self.aspect = aspect;
        self.apply(queue);
        true
    }

    /// Updates how much of the widget the model fills; returns whether the framing
    /// changed.
    pub fn set_fill(&mut self, queue: &Queue, fill: f32) -> bool {
        let fill = sane_fill(fill);

        if (self.fill - fill).abs() < TOLERANCE {
            return false;
        }

        self.fill = fill;
        self.apply(queue);
        true
    }

    /// The aspect ratio this view was framed for.
    pub fn aspect(&self) -> f32 {
        self.aspect
    }

    /// How much of the widget the model fills.
    pub fn fill(&self) -> f32 {
        self.fill
    }

    /// The model-space box this view frames.
    pub fn bounds(&self) -> ModelBounds {
        self.bounds
    }

    /// The model-space to clip-space matrix this view uses.
    pub fn matrix(&self) -> &Matrix44 {
        &self.matrix
    }

    /// The matrix the GPU draws with.
    pub fn transform(&self) -> &WgpuTransform {
        &self.transform
    }

    /// Maps a clip-space point (`-1.0..=1.0`, `+y` up) back to model space — the
    /// exact inverse of what the vertex shader applies.
    pub fn to_model(&self, point: [f32; 2]) -> [f32; 2] {
        [
            self.matrix.invert_transform_x(point[0]),
            self.matrix.invert_transform_y(point[1]),
        ]
    }

    /// Maps a model-space point to clip space.
    pub fn to_clip(&self, point: [f32; 2]) -> [f32; 2] {
        [
            self.matrix.transform_x(point[0]),
            self.matrix.transform_y(point[1]),
        ]
    }

    fn apply(&mut self, queue: &Queue) {
        self.matrix = self.bounds.fit_matrix(self.aspect, self.fill * NDC);
        self.transform.update_matrix(queue, &self.matrix);
    }
}

fn sane_aspect(aspect: f32) -> f32 {
    if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    }
}

fn sane_fill(fill: f32) -> f32 {
    if fill.is_finite() && fill > 0.0 {
        fill
    } else {
        1.0
    }
}
