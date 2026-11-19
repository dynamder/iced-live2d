use std::ops::Range;
use std::sync::Arc;

use wgpu::util::DeviceExt;

use crate::moc3::{Moc3DrawableBlendMode, Moc3DrawableMesh};
use crate::render::common::{
    ClippingRect, DrawableInfo, DrawableVertex, draw_order_indices_from, encode_indices,
    encode_vertices_from_drawable,
};

/// Smallest shared vertex buffer worth allocating, in bytes.
const MIN_VERTEX_BUFFER_BYTES: u64 = 4;

pub fn drawable_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 5] = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 8,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 16,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 20,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 32,
            shader_location: 4,
        },
    ];

    wgpu::VertexBufferLayout {
        array_stride: DrawableVertex::STRIDE as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRIBUTES,
    }
}

#[derive(Debug)]
pub struct WgpuDrawableBuffers {
    vertex_buffer: Arc<wgpu::Buffer>,
    vertex_span: Range<u64>,
    index_buffer: wgpu::Buffer,
    vertex_count: u32,
    index_count: u32,
    vertex_bytes: Vec<u8>,
    indices: Vec<u16>,
    info: DrawableInfo,
}

impl WgpuDrawableBuffers {
    /// The part of the shared vertex buffer that holds this drawable.
    ///
    /// Drawables share one buffer so a frame's worth of geometry can be written
    /// with a single `write_buffer` call; per-drawable buffers cost one copy
    /// command each and dominate the frame on rigs with hundreds of drawables.
    pub fn vertex_buffer(&self) -> wgpu::BufferSlice<'_> {
        self.vertex_buffer.slice(self.vertex_span.clone())
    }

    pub fn vertex_offset(&self) -> u64 {
        self.vertex_span.start
    }

    pub fn index_buffer(&self) -> &wgpu::Buffer {
        &self.index_buffer
    }

    pub fn index_count(&self) -> u32 {
        self.index_count
    }

    pub fn is_empty(&self) -> bool {
        self.vertex_count == 0 || self.index_count == 0
    }

    pub fn is_visible(&self) -> bool {
        !self.is_empty() && self.info.is_visible()
    }

    pub fn info(&self) -> &DrawableInfo {
        &self.info
    }

    pub fn texture_index(&self) -> i32 {
        self.info.texture_index()
    }

    pub fn blend_mode(&self) -> Moc3DrawableBlendMode {
        self.info.blend_mode()
    }

    pub fn opacity(&self) -> f32 {
        self.info.opacity()
    }

    pub fn draw_order(&self) -> f32 {
        self.info.draw_order()
    }

    pub fn render_order(&self) -> i32 {
        self.info.render_order()
    }

    pub fn masks(&self) -> &[i32] {
        self.info.masks()
    }

    pub fn inverted_mask(&self) -> bool {
        self.info.inverted_mask()
    }

    pub fn bounds(&self) -> Option<ClippingRect> {
        self.info.bounds()
    }
}

#[derive(Debug)]
pub struct WgpuMeshBuffers {
    vertex_buffer: Arc<wgpu::Buffer>,
    drawables: Vec<WgpuDrawableBuffers>,
    draw_order_indices: Vec<usize>,
    /// Staging bytes for the last upload, kept so a frame does not allocate and
    /// free a few megabytes.
    upload: Vec<u8>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct WgpuMeshUpdate {
    uploaded_drawables: usize,
    bounds_changed: bool,
    visibility_changed: bool,
    structure_changed: bool,
}

impl WgpuMeshUpdate {
    pub fn uploaded_drawables(&self) -> usize {
        self.uploaded_drawables
    }

    pub fn bounds_changed(&self) -> bool {
        self.bounds_changed
    }

    pub fn visibility_changed(&self) -> bool {
        self.visibility_changed
    }

    /// Whether a drawable's mask set, inverted-mask flag, texture or blend mode
    /// changed. None of those touch the vertex or index buffers, but the clipping
    /// resources were built from the old values and have to be rebuilt.
    pub fn structure_changed(&self) -> bool {
        self.structure_changed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WgpuMeshUpdateError {
    #[error("drawable count changed from {expected} to {actual}")]
    DrawableCount { expected: usize, actual: usize },
    #[error("drawable {drawable_index} vertex count changed from {expected} to {actual}")]
    VertexCount {
        drawable_index: usize,
        expected: usize,
        actual: usize,
    },
    #[error("drawable {drawable_index} index count changed from {expected} to {actual}")]
    IndexCount {
        drawable_index: usize,
        expected: usize,
        actual: usize,
    },
    #[error("drawable {drawable_index} indices changed")]
    Indices { drawable_index: usize },
    #[error("drawable {drawable_index} texture index changed from {expected} to {actual}")]
    TextureIndex {
        drawable_index: usize,
        expected: i32,
        actual: i32,
    },
    #[error("drawable {drawable_index} blend mode changed from {expected:?} to {actual:?}")]
    BlendMode {
        drawable_index: usize,
        expected: Moc3DrawableBlendMode,
        actual: Moc3DrawableBlendMode,
    },
    #[error("drawable {drawable_index} masks changed")]
    Masks { drawable_index: usize },
    #[error("drawable {drawable_index} inverted mask changed from {expected} to {actual}")]
    InvertedMask {
        drawable_index: usize,
        expected: bool,
        actual: bool,
    },
}

impl WgpuMeshBuffers {
    pub fn from_drawables(device: &wgpu::Device, meshes: &[Moc3DrawableMesh]) -> Option<Self> {
        let spans = vertex_spans(meshes)?;
        let total = spans.last().map_or(0, |span| span.end);
        let vertex_buffer = Arc::new(create_shared_vertex_buffer(device, total));

        let mut drawables = Vec::with_capacity(meshes.len());
        for (mesh, span) in meshes.iter().zip(spans) {
            drawables.push(create_drawable_buffers(
                device,
                Arc::clone(&vertex_buffer),
                span,
                mesh,
            )?);
        }

        fill_vertex_buffer(&vertex_buffer, &drawables)?;

        let draw_order_indices = draw_order_indices_from(
            drawables.len(),
            |index| drawables[index].draw_order(),
            |index| drawables[index].render_order(),
        );

        Some(Self {
            vertex_buffer,
            drawables,
            draw_order_indices,
            upload: Vec::new(),
        })
    }

    pub fn drawables(&self) -> &[WgpuDrawableBuffers] {
        &self.drawables
    }

    pub fn drawable_infos(&self) -> Vec<DrawableInfo> {
        self.drawables.iter().map(|d| d.info.clone()).collect()
    }

    pub(crate) fn iter_drawable_infos(&self) -> impl Iterator<Item = &DrawableInfo> {
        self.drawables.iter().map(WgpuDrawableBuffers::info)
    }

    pub(crate) fn drawable_bounds(&self, drawable_index: usize) -> Option<ClippingRect> {
        self.drawables
            .get(drawable_index)
            .and_then(WgpuDrawableBuffers::bounds)
    }

    pub fn draw_order_indices(&self) -> &[usize] {
        &self.draw_order_indices
    }

    pub fn update_drawables(
        &mut self,
        queue: &wgpu::Queue,
        meshes: &[Moc3DrawableMesh],
    ) -> Result<WgpuMeshUpdate, WgpuMeshUpdateError> {
        if self.drawables.len() != meshes.len() {
            return Err(WgpuMeshUpdateError::DrawableCount {
                expected: self.drawables.len(),
                actual: meshes.len(),
            });
        }

        for (drawable_index, (drawable, mesh)) in self.drawables.iter().zip(meshes).enumerate() {
            validate_drawable_update(drawable_index, drawable, mesh)?;
        }

        let mut uploads = 0;
        let mut bounds_changed = false;
        let mut visibility_changed = false;
        let mut structure_changed = false;
        let mut dirty: Option<Range<usize>> = None;
        for (drawable_index, (drawable, mesh)) in self.drawables.iter_mut().zip(meshes).enumerate() {
            if renderer_vertex_data_changed(drawable, mesh) {
                drawable.vertex_bytes.clear();
                encode_vertices_from_drawable(mesh, &mut drawable.vertex_bytes);
                dirty = Some(match dirty {
                    Some(range) => range.start..drawable_index + 1,
                    None => drawable_index..drawable_index + 1,
                });
                uploads += 1;
            }
            let was_visible = drawable.is_visible();
            let info = DrawableInfo::from_mesh(mesh);
            let is_visible = !drawable.is_empty() && info.is_visible();
            bounds_changed |= drawable.info.bounds() != info.bounds();
            visibility_changed |= was_visible != is_visible;
            structure_changed |= drawable.info.masks() != info.masks()
                || drawable.info.inverted_mask() != info.inverted_mask()
                || drawable.info.texture_index() != info.texture_index()
                || drawable.info.blend_mode() != info.blend_mode();
            drawable.info = info;
        }

        if let Some(range) = dirty {
            self.write_vertex_range(queue, range);
        }

        self.draw_order_indices = draw_order_indices_from(
            self.drawables.len(),
            |index| self.drawables[index].draw_order(),
            |index| self.drawables[index].render_order(),
        );

        Ok(WgpuMeshUpdate {
            uploaded_drawables: uploads,
            bounds_changed,
            visibility_changed,
            structure_changed,
        })
    }
}

/// Byte span of every drawable's vertices inside the shared buffer.
///
/// Drawables are packed back to back in drawable order, so any run of them is
/// contiguous and can be written with one call.
fn vertex_spans(meshes: &[Moc3DrawableMesh]) -> Option<Vec<Range<u64>>> {
    let mut spans = Vec::with_capacity(meshes.len());
    let mut offset = 0u64;

    for mesh in meshes {
        let bytes = u64::try_from(mesh.vertices().len()).ok()? * DrawableVertex::STRIDE as u64;
        spans.push(offset..offset + bytes);
        offset += bytes;
    }

    Some(spans)
}

fn create_shared_vertex_buffer(device: &wgpu::Device, bytes: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("live2d.drawable.vertices"),
        size: bytes.max(MIN_VERTEX_BUFFER_BYTES),
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: true,
    })
}

/// Copies every drawable's cached bytes into the shared buffer, which is created
/// mapped for exactly this purpose.
fn fill_vertex_buffer(
    vertex_buffer: &wgpu::Buffer,
    drawables: &[WgpuDrawableBuffers],
) -> Option<()> {
    let mut mapped = vertex_buffer.slice(..).get_mapped_range_mut().ok()?;

    for drawable in drawables {
        let start = drawable.vertex_offset() as usize;
        let end = start + drawable.vertex_bytes.len();
        mapped.slice(start..end).copy_from_slice(&drawable.vertex_bytes);
    }

    drop(mapped);
    vertex_buffer.unmap();

    Some(())
}

impl WgpuMeshBuffers {
    /// Writes the drawables in `range` to the shared buffer with one call.
    ///
    /// Writing each drawable on its own is what makes a heavy rig expensive:
    /// several hundred `write_buffer` calls per frame, each one a separate
    /// staging copy and copy command.
    fn write_vertex_range(&mut self, queue: &wgpu::Queue, range: Range<usize>) {
        let start = self.drawables[range.start].vertex_span.start;
        let end = self.drawables[range.end - 1].vertex_span.end;

        let Self {
            vertex_buffer,
            drawables,
            upload,
            ..
        } = self;

        upload.clear();
        upload.reserve((end - start) as usize);

        for drawable in &drawables[range] {
            upload.extend_from_slice(&drawable.vertex_bytes);
        }

        if !upload.is_empty() {
            queue.write_buffer(vertex_buffer, start, upload.as_slice());
        }
    }
}

fn validate_drawable_update(
    drawable_index: usize,
    drawable: &WgpuDrawableBuffers,
    mesh: &Moc3DrawableMesh,
) -> Result<(), WgpuMeshUpdateError> {
    validate_count(
        drawable.vertex_count as usize,
        mesh.vertices().len(),
        WgpuMeshUpdateError::VertexCount {
            drawable_index,
            expected: drawable.vertex_count as usize,
            actual: mesh.vertices().len(),
        },
    )?;
    validate_count(
        drawable.index_count as usize,
        mesh.indices().len(),
        WgpuMeshUpdateError::IndexCount {
            drawable_index,
            expected: drawable.index_count as usize,
            actual: mesh.indices().len(),
        },
    )?;
    validate_unchanged(
        drawable.indices.as_slice(),
        mesh.indices(),
        WgpuMeshUpdateError::Indices { drawable_index },
    )?;

    Ok(())
}

fn validate_count(
    expected: usize,
    actual: usize,
    error: WgpuMeshUpdateError,
) -> Result<(), WgpuMeshUpdateError> {
    if expected == actual {
        Ok(())
    } else {
        Err(error)
    }
}

fn validate_unchanged<T: PartialEq + ?Sized>(
    expected: &T,
    actual: &T,
    error: WgpuMeshUpdateError,
) -> Result<(), WgpuMeshUpdateError> {
    if expected == actual {
        Ok(())
    } else {
        Err(error)
    }
}

pub fn create_wgpu_drawable_buffers(
    device: &wgpu::Device,
    mesh: &Moc3DrawableMesh,
) -> Option<WgpuDrawableBuffers> {
    let mut vertex_bytes = Vec::new();
    encode_vertices_from_drawable(mesh, &mut vertex_bytes);

    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("live2d.drawable.vertices"),
        contents: &vertex_bytes,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
    });
    let span = 0..u64::try_from(vertex_bytes.len()).ok()?;

    create_drawable_buffers(device, Arc::new(vertex_buffer), span, mesh)
}

/// Builds the per-drawable GPU state for `mesh`, whose vertices live at
/// `vertex_span` inside a shared vertex buffer.
fn create_drawable_buffers(
    device: &wgpu::Device,
    vertex_buffer: Arc<wgpu::Buffer>,
    vertex_span: Range<u64>,
    mesh: &Moc3DrawableMesh,
) -> Option<WgpuDrawableBuffers> {
    let mut vertex_bytes = Vec::new();
    encode_vertices_from_drawable(mesh, &mut vertex_bytes);
    let index_bytes = encode_indices(mesh.indices());
    let vertex_count = u32::try_from(mesh.vertices().len()).ok()?;
    let index_count = u32::try_from(mesh.indices().len()).ok()?;

    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("live2d.drawable.indices"),
        contents: &index_bytes,
        usage: wgpu::BufferUsages::INDEX,
    });

    Some(WgpuDrawableBuffers {
        vertex_buffer,
        vertex_span,
        index_buffer,
        vertex_count,
        index_count,
        vertex_bytes,
        indices: mesh.indices().to_vec(),
        info: DrawableInfo::from_mesh(mesh),
    })
}

fn renderer_vertex_data_changed(drawable: &WgpuDrawableBuffers, mesh: &Moc3DrawableMesh) -> bool {
    if drawable.vertex_count as usize != mesh.vertices().len() {
        return true;
    }
    if drawable.vertex_bytes.len() != mesh.vertices().len() * DrawableVertex::STRIDE {
        return true;
    }

    let opacity = mesh.opacity().to_ne_bytes();
    let multiply = color_bytes(mesh.multiply_color());
    let screen = color_bytes(mesh.screen_color());
    drawable
        .vertex_bytes
        .as_chunks::<{ DrawableVertex::STRIDE }>()
        .0
        .iter()
        .zip(mesh.vertices())
        .any(|(bytes, vertex)| {
            bytes[0..8] != vec2_bytes(vertex.position())
                || bytes[8..16] != vec2_bytes(vertex.uv())
                || bytes[16..20] != opacity
                || bytes[20..32] != multiply
                || bytes[32..44] != screen
        })
}

fn vec2_bytes(values: [f32; 2]) -> [u8; 8] {
    let mut bytes = [0; 8];
    bytes[0..4].copy_from_slice(&values[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&values[1].to_ne_bytes());
    bytes
}

fn color_bytes(values: [f32; 3]) -> [u8; 12] {
    let mut bytes = [0; 12];
    bytes[0..4].copy_from_slice(&values[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&values[1].to_ne_bytes());
    bytes[8..12].copy_from_slice(&values[2].to_ne_bytes());
    bytes
}
