//! The smallest correct driver of the render layer: one asset, one instance, one
//! view, stepped `advance` → `deform` → `upload`.
//!
//! The examples do not all use every hook, so this module allows unused helpers.
#![allow(dead_code)]

use std::sync::Arc;

use live2d_render::wgpu;
use live2d_render::{
    Asset, AssetId, AssetOptions, ClipRect, Error, Model, ModelGpu, ModelInfo, ModelOptions, View,
    WgpuLive2dRenderer,
};

/// The framing the sample models were verified against: 0.925 of the widget.
pub const DEFAULT_FILL: f32 = 0.925;

pub struct Session {
    renderer: WgpuLive2dRenderer,
    asset: Asset,
    model: Model,
    gpu: ModelGpu,
    view: View,
    issues: Vec<Error>,
}

impl Session {
    pub fn load(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &str,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        options: AssetOptions,
        model_options: ModelOptions,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let renderer = WgpuLive2dRenderer::new(device, format);
        let prepared = Asset::parse(AssetId::unique(), path, options)?;
        let mut asset = prepared.upload(&renderer, device, queue, options)?;
        let issues = asset.take_issues();
        let model = asset.model();
        let gpu = ModelGpu::new(&renderer, device, &model, model_options.mask_size)?;
        let view = View::new(
            &renderer,
            device,
            asset.info().bounds,
            size.0 as f32 / size.1.max(1) as f32,
            DEFAULT_FILL,
        );

        Ok(Self {
            renderer,
            asset,
            model,
            gpu,
            view,
            issues,
        })
    }

    pub fn info(&self) -> &Arc<ModelInfo> {
        self.asset.info()
    }

    pub fn model(&mut self) -> &mut Model {
        &mut self.model
    }

    /// Which hit area a clip-space point (`-1.0..=1.0`, `+y` up) lands in, through
    /// the view's inverse transform.
    pub fn hit_area_at(&self, point: [f32; 2]) -> Option<&str> {
        let model_point = self.view.to_model(point);
        let index = self.model.hit_area(model_point)?;

        self.asset
            .info()
            .hit_areas
            .get(index.get())
            .map(|area| &*area.name)
    }

    /// Overrides the default framing.
    pub fn set_fill(&mut self, queue: &wgpu::Queue, fill: f32) {
        self.view.set_fill(queue, fill);
    }

    /// Runs the update order and pushes the result to the GPU.
    pub fn advance(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        delta: std::time::Duration,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if self.model.advance(delta) == 0 {
            return Ok(());
        }

        if !self.model.deform() {
            return Err(Error::MeshUpdate.into());
        }

        if let Some(problem) = self
            .gpu
            .upload(&self.renderer, device, queue, &self.model)?
        {
            self.issues.push(problem);
        }

        Ok(())
    }

    pub fn rebuild_clipping(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Err(error) = self.gpu.rebuild_clipping(&self.renderer, device, queue) {
            self.issues.push(error);
        }
    }

    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip: ClipRect,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.gpu.encode(
            &self.renderer,
            encoder,
            target,
            clip,
            self.asset.textures(),
            &self.view,
        )?;

        Ok(())
    }

    pub fn take_issues(&mut self) -> Vec<Error> {
        self.issues.extend(self.model.take_issues());

        std::mem::take(&mut self.issues)
    }
}
