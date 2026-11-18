//! Model-space bounds and the view-fit matrix derived from them.

use mocari::core::Matrix44;
use mocari::moc3::{Moc3CanvasInfo, Moc3DrawableMesh};

/// Axis-aligned model-space rectangle used to frame the model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelBounds {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

impl ModelBounds {
    /// The artist-authored canvas from the `.moc3` header.
    ///
    /// This is the framing Cubism itself uses, so it is stable across parameter
    /// changes and ignores the alternate parts (spare hands, props, extra
    /// outfits) that a "bounding box of every drawable" would pull in.
    ///
    /// Returns `None` for degenerate canvases and for models that flip their Y
    /// axis, where recomputing from drawables is the safer fallback.
    pub fn from_canvas(canvas: &Moc3CanvasInfo) -> Option<Self> {
        let pixels_per_unit = canvas.pixels_per_unit();
        let width = canvas.width() / pixels_per_unit;
        let height = canvas.height() / pixels_per_unit;

        if !(pixels_per_unit > 0.0) || !(width > 0.0) || !(height > 0.0) {
            return None;
        }

        if canvas.reverse_y_coordinate() {
            return None;
        }

        let origin_x = canvas.origin_x() / pixels_per_unit;
        let origin_y = canvas.origin_y() / pixels_per_unit;

        Some(Self {
            min_x: -origin_x,
            min_y: -origin_y,
            max_x: width - origin_x,
            max_y: height - origin_y,
        })
    }

    /// Bounding box of every drawable vertex, in model space.
    ///
    /// Returns `None` when the model has no vertices or a degenerate box.
    pub fn from_drawables(drawables: &[Moc3DrawableMesh]) -> Option<Self> {
        let mut bounds: Option<Self> = None;

        for vertex in drawables.iter().flat_map(Moc3DrawableMesh::vertices) {
            let [x, y] = vertex.position();
            bounds = Some(match bounds {
                Some(bounds) => Self {
                    min_x: bounds.min_x.min(x),
                    min_y: bounds.min_y.min(y),
                    max_x: bounds.max_x.max(x),
                    max_y: bounds.max_y.max(y),
                },
                None => Self {
                    min_x: x,
                    min_y: y,
                    max_x: x,
                    max_y: y,
                },
            });
        }

        bounds.filter(|bounds| bounds.width() > 0.0 && bounds.height() > 0.0)
    }

    pub fn width(&self) -> f32 {
        self.max_x - self.min_x
    }

    pub fn height(&self) -> f32 {
        self.max_y - self.min_y
    }

    pub fn center(&self) -> [f32; 2] {
        [
            (self.min_x + self.max_x) / 2.0,
            (self.min_y + self.max_y) / 2.0,
        ]
    }

    /// Builds the model-space -> clip-space matrix that fits the model inside a
    /// viewport of the given aspect ratio.
    ///
    /// `view_fill` is the fraction of the NDC range (2.0) the model may occupy:
    /// 2.0 fills exactly, 1.85 leaves a small margin.
    pub fn fit_matrix(&self, aspect: f32, view_fill: f32) -> Matrix44 {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };

        let fit_x = view_fill / (self.width() * aspect);
        let fit_y = view_fill / self.height();
        let scale_y = fit_x.min(fit_y);
        let scale_x = scale_y / aspect;

        let [center_x, center_y] = self.center();

        let mut matrix = Matrix44::identity();
        matrix.scale(scale_x, scale_y);
        matrix.translate(-center_x * scale_x, -center_y * scale_y);
        matrix
    }
}
