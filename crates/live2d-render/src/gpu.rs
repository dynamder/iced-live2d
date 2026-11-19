//! The GPU mirror of one instance: mesh buffers, clipping resources and the mask
//! render target.
//!
//! Everything here needs a `device`; the CPU side ([`Model`](crate::Model)) does
//! not. That split is what lets the whole update order be tested without a GPU.

use mocari::render::wgpu::{
    WgpuClippingPlan, WgpuClippingResources, WgpuLive2dRenderer, WgpuMaskRenderTarget,
    WgpuMeshBuffers, WgpuTexture,
};
use wgpu::{CommandEncoder, Device, Queue, TextureView};

use crate::error::{Error, Result};
use crate::model::Model;
use crate::view::{ClipRect, View};

/// The upper bound [`crate::ModelOptions::mask_size`] may ask for, in case a host
/// passes something the GPU cannot allocate.
const MIN_MASK_SIZE: u32 = 64;

/// What a drawable set needs on the GPU, rebuilt from the CPU meshes.
pub struct ModelGpu {
    mesh: WgpuMeshBuffers,
    clipping: WgpuClippingResources,
    mask: WgpuMaskRenderTarget,
}

impl ModelGpu {
    /// Allocates every GPU resource this instance needs.
    pub fn new(
        renderer: &WgpuLive2dRenderer,
        device: &Device,
        model: &Model,
        mask_size: u32,
    ) -> Result<Self> {
        let mesh =
            WgpuMeshBuffers::from_drawables(device, model.meshes()).ok_or(Error::MeshBuffers)?;

        let mut plan = WgpuClippingPlan::from_mesh_buffers(&mesh);
        plan.prepare_single_texture_masks(&mesh)?;
        let clipping = renderer.create_clipping_resources(device, &plan)?;
        let mask = renderer.create_mask_render_target(device, mask_size.max(MIN_MASK_SIZE))?;

        Ok(Self {
            mesh,
            clipping,
            mask,
        })
    }

    /// Pushes the current meshes to the GPU.
    ///
    /// The returned error, when there is one, is *why* the whole buffer set had to
    /// be rebuilt: a drawable changed shape in a way the buffers cannot absorb.
    /// It is worth reporting, but the instance keeps drawing.
    pub fn upload(
        &mut self,
        renderer: &WgpuLive2dRenderer,
        device: &Device,
        queue: &Queue,
        model: &Model,
    ) -> Result<Option<Error>> {
        let meshes = model.meshes();

        let (needs_reclip, rejected) = match self.mesh.update_drawables(queue, meshes) {
            Ok(update) => (
                update.bounds_changed()
                    || update.visibility_changed()
                    || update.structure_changed(),
                None,
            ),
            // A drawable changed shape in a way the buffers cannot absorb, so the
            // whole set is rebuilt. Correct but expensive; a model doing it every
            // frame looks exactly like a slow one.
            Err(error) => {
                self.mesh =
                    WgpuMeshBuffers::from_drawables(device, meshes).ok_or(Error::MeshBuffers)?;
                (true, Some(Error::from(error)))
            }
        };

        if needs_reclip {
            self.rebuild_clipping(renderer, device, queue)?;
        }

        Ok(rejected)
    }

    /// Records the mask pass and the visible pass into `encoder`.
    ///
    /// Masks are drawn into the instance's own off-screen buffer with the full
    /// viewport: the Live2D mask shader works in the same clip space as the
    /// visible pass, so the buffer must keep its own full extent. The visible
    /// pass is scissored and viewported to `clip`.
    pub fn encode(
        &self,
        renderer: &WgpuLive2dRenderer,
        encoder: &mut CommandEncoder,
        target: &TextureView,
        clip: ClipRect,
        textures: &[WgpuTexture],
        view: &View,
    ) -> Result<()> {
        if clip.is_empty() {
            return Ok(());
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("live2d.mask_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.mask.view(),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            renderer.draw_masks_with_textures(&mut pass, &self.mesh, &self.clipping, textures)?;
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("live2d.model_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_viewport(
                clip.x as f32,
                clip.y as f32,
                clip.width as f32,
                clip.height as f32,
                0.0,
                1.0,
            );
            pass.set_scissor_rect(clip.x, clip.y, clip.width, clip.height);

            renderer.draw_with_textures_clipping_and_transform(
                &mut pass,
                &self.mesh,
                textures,
                &self.clipping,
                &self.mask,
                view.transform(),
            )?;
        }

        Ok(())
    }

    /// Rebuilds clipping resources unconditionally.
    ///
    /// Diagnostic hook: comparing a run that calls this every frame against one
    /// that does not isolates "stale mask layout" from "wrong mask math".
    pub fn rebuild_clipping(
        &mut self,
        renderer: &WgpuLive2dRenderer,
        device: &Device,
        queue: &Queue,
    ) -> Result<()> {
        let mut plan = WgpuClippingPlan::from_mesh_buffers(&self.mesh);
        plan.prepare_single_texture_masks(&self.mesh)?;

        if !renderer.update_clipping_resources(queue, &mut self.clipping, &plan)? {
            self.clipping = renderer.create_clipping_resources(device, &plan)?;
        }

        Ok(())
    }
}
