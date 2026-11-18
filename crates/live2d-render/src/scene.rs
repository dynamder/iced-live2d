//! The frame-to-frame Live2D scene: CPU playback state plus its GPU mirror.
//!
//! Everything here is UI-agnostic. A host (iced, winit, anything else) only has
//! to call [`Live2dScene::advance`] once per frame and [`Live2dScene::encode`]
//! with an encoder that targets its own surface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mocari::assets::load_model_runtime;
use mocari::expression::load_expression;
use mocari::motion::load_motion;
use mocari::render::wgpu::{
    WgpuClippingPlan, WgpuClippingResources, WgpuLive2dRenderer, WgpuMaskRenderTarget,
    WgpuMeshBuffers, WgpuTexture, WgpuTransform,
};
use mocari::{ExpressionManager, ModelRuntime, MotionPlayer};
use wgpu::{CommandEncoder, Device, Queue, TextureFormat, TextureView};

use crate::bounds::ModelBounds;
use crate::error::{Error, Result};

/// Size of the single shared mask render target.
///
/// Cubism models draw every clipped drawable into this off-screen buffer before
/// the visible pass. Bigger costs VRAM but keeps mask edges crisp on 4096px
/// textures.
pub const MASK_TARGET_SIZE: u32 = 1024;

const DEFAULT_VIEW_FILL: f32 = 1.85;
/// Longest animation step applied in one frame, so a stalled window does not
/// teleport the motion timeline.
const MAX_STEP_SECONDS: f32 = 0.1;
const IDLE_GROUP: &str = "Idle";
/// Group name given to motions discovered by scanning the model directory when
/// `model3.json` declares none.
const SCANNED_GROUP: &str = "Scanned";
const MAX_SCAN_DEPTH: usize = 2;

/// Loading knobs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneOptions {
    /// Texture resolution multiplier applied when uploading.
    ///
    /// 1.0 keeps the authored resolution (a 5-texture rig with one 8192x8192
    /// atlas costs ~592 MB of VRAM). 0.5 quarters that, which is plenty for a
    /// few-hundred-pixel desktop pet.
    pub texture_scale: f32,
    /// Fraction of the NDC range the model may occupy. 2.0 fills exactly.
    pub view_fill: f32,
    /// Edge length of the shared mask render target. Raise it if a complex rig
    /// shows clipped geometry — packed mask contexts can run out of atlas space.
    pub mask_size: u32,
}

impl Default for SceneOptions {
    fn default() -> Self {
        Self {
            texture_scale: 0.5,
            view_fill: DEFAULT_VIEW_FILL,
            mask_size: MASK_TARGET_SIZE,
        }
    }
}

/// How the gaze driver feeds the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GazeInputs {
    /// VTuber-style rig: write AngleX/Y/Z, Position* and LeanInput.
    Tracking,
    /// Classic rig: write ParamAngle* directly.
    Model,
}

/// A scissor/viewport rectangle in physical target pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ClipRect {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn aspect(&self) -> f32 {
        self.width as f32 / self.height.max(1) as f32
    }
}

struct GpuState {
    mesh_buffers: WgpuMeshBuffers,
    textures: Vec<WgpuTexture>,
    clipping_resources: WgpuClippingResources,
    mask_target: WgpuMaskRenderTarget,
    transform: WgpuTransform,
}

pub struct Live2dScene {
    renderer: WgpuLive2dRenderer,
    model: ModelRuntime,
    motion_groups: BTreeMap<String, Vec<PathBuf>>,
    expression_paths: Vec<PathBuf>,
    player: Option<MotionPlayer>,
    expressions: ExpressionManager,
    /// Set when something outside the animation timeline changed (gaze, model
    /// swap) so the next frame rebuilds meshes even if no motion is playing.
    dirty: bool,
    gpu: GpuState,
    bounds: ModelBounds,
    view_fill: f32,
    fit_aspect: f32,
    motion_cursor: usize,
    gaze_inputs: GazeInputs,
    physics_enabled: bool,
    profile: bool,
    profile_frames: u32,
    profile_cpu: f64,
    profile_upload: f64,
    expression_cursor: usize,
}

impl Live2dScene {
    /// Loads a `.model3.json`, uploads its textures and allocates the per-model
    /// GPU state.
    ///
    /// `color_format` must be the format of the render target the scene will
    /// draw into; the Live2D pipelines are built for it.
    pub fn load(
        device: &Device,
        queue: &Queue,
        model_path: impl AsRef<Path>,
        color_format: TextureFormat,
    ) -> Result<Self> {
        Self::load_with_options(device, queue, model_path, color_format, SceneOptions::default())
    }

    pub fn load_with_options(
        device: &Device,
        queue: &Queue,
        model_path: impl AsRef<Path>,
        color_format: TextureFormat,
        options: SceneOptions,
    ) -> Result<Self> {
        let renderer = WgpuLive2dRenderer::new(device, color_format);

        let loaded = load_model_runtime(model_path.as_ref())?;
        let model_dir = loaded.model_dir().map(Path::to_path_buf);
        // Cloning the runtime and dropping `loaded` frees the decoded PNG bytes.
        // Five 4096x4096 textures is 335 MB of CPU memory we never need again
        // once they are on the GPU.
        let model = loaded.runtime().clone();

        let bounds = ModelBounds::from_canvas(&model.canvas())
            .or_else(|| ModelBounds::from_drawables(model.meshes()))
            .ok_or(Error::NoDrawables)?;

        let factor = downscale_factor(options.texture_scale);
        let mut uploaded_bytes = 0usize;

        let textures = loaded
            .textures()
            .iter()
            .map(|texture| {
                let (width, height, rgba) = if factor > 1 {
                    downscale_rgba(
                        texture.width(),
                        texture.height(),
                        texture.rgba(),
                        factor,
                    )
                } else {
                    (
                        texture.width(),
                        texture.height(),
                        texture.rgba().to_vec(),
                    )
                };

                uploaded_bytes += rgba.len();

                renderer
                    .create_rgba8_texture(device, queue, width, height, &rgba)
                    .map_err(Error::from)
            })
            .collect::<Result<Vec<_>>>()?;

        println!(
            "live2d: {} textures, {} MB uploaded (scale {:.2})",
            textures.len(),
            uploaded_bytes / (1024 * 1024),
            options.texture_scale,
        );

        let mesh_buffers = WgpuMeshBuffers::from_drawables(device, model.meshes())
            .ok_or(Error::MeshBuffers)?;

        let clipping_resources = {
            let mut plan = WgpuClippingPlan::from_mesh_buffers(&mesh_buffers);
            plan.prepare_single_texture_masks(&mesh_buffers)?;
            renderer.create_clipping_resources(device, &plan)?
        };

        let mask_target = renderer.create_mask_render_target(device, options.mask_size.max(64))?;
        let transform = renderer.create_transform(device, &bounds.fit_matrix(1.0, options.view_fill));

        // Rigs exported for VTuber software drive tracking through AngleX/Y/Z,
        // PositionX/Y/Z and LeanInput; physics then derives ParamAngle*,
        // ParamBodyAngle* and the rest. Writing ParamAngle* directly on such a
        // rig is useless: those are physics outputs and are overwritten later in
        // the same frame.
        let gaze_inputs = if model.parameter_index("AngleX").is_some() {
            GazeInputs::Tracking
        } else {
            GazeInputs::Model
        };

        let mut motion_groups = motion_paths_by_group(&model, model_dir.as_deref());
        let mut expression_paths = expression_paths(&model, model_dir.as_deref());

        // Everything below only needs the GPU copies.
        drop(loaded);

        // Plenty of exported models (VTuber rigs in particular) ship their
        // motion and expression files next to the model without listing them in
        // `model3.json`. Fall back to scanning the model directory.
        if let Some(model_dir) = model_dir.as_deref()
            && (motion_groups.is_empty() || expression_paths.is_empty())
        {
            let scanned = scan_sidecars(model_dir);

            if motion_groups.is_empty() && !scanned.motions.is_empty() {
                motion_groups.insert(SCANNED_GROUP.to_owned(), scanned.motions);
            }

            if expression_paths.is_empty() {
                expression_paths = scanned.expressions;
            }
        }

        let mut scene = Self {
            renderer,
            model,
            motion_groups,
            expression_paths,
            player: None,
            expressions: ExpressionManager::new(),
            dirty: true,
            gpu: GpuState {
                mesh_buffers,
                textures,
                clipping_resources,
                mask_target,
                transform,
            },
            bounds,
            view_fill: options.view_fill,
            fit_aspect: 1.0,
            motion_cursor: 0,
            gaze_inputs,
            physics_enabled: true,
            profile: std::env::var_os("LIVE2D_PROFILE").is_some(),
            profile_frames: 0,
            profile_cpu: 0.0,
            profile_upload: 0.0,
            expression_cursor: 0,
        };

        scene.play_next_motion();
        Ok(scene)
    }

    /// Advances the animation by `delta` and uploads any changed geometry.
    pub fn advance(&mut self, device: &Device, queue: &Queue, delta: Duration) -> Result<()> {
        let delta = delta.as_secs_f32().clamp(0.0, MAX_STEP_SECONDS);

        let animating = self.player.is_some()
            || self.expressions.active_expression_count() > 0
            || self.model.physics().is_some();

        if !self.dirty && !animating {
            return Ok(());
        }

        let mut finished_motion = false;
        let cpu_start = std::time::Instant::now();

        {
            let runtime = &mut self.model;

            runtime.reset_parameters();
            runtime.reset_part_opacities();

            if let Some(player) = self.player.as_mut() {
                player.tick(delta);
                player.apply(runtime);
                finished_motion = player.is_finished();
            }

            self.expressions.tick(delta);
            self.expressions.apply(runtime);

            // Overrides (gaze, user sliders) win over motion and expression.
            runtime.apply_parameter_overrides();

            if self.physics_enabled {
                runtime.apply_physics(delta);
            }
            runtime.apply_pose(delta);

            if runtime.update_meshes().is_none() {
                return Err(Error::MeshUpdate);
            }
        }

        let cpu_elapsed = cpu_start.elapsed().as_secs_f64() * 1000.0;

        if finished_motion {
            self.player = None;
            self.play_next_motion();
        }

        self.dirty = false;

        let upload_start = std::time::Instant::now();
        self.upload(device, queue)?;
        let upload_elapsed = upload_start.elapsed().as_secs_f64() * 1000.0;

        if self.profile {
            self.profile_frames += 1;
            self.profile_cpu += cpu_elapsed;
            self.profile_upload += upload_elapsed;

            if self.profile_frames >= 30 {
                println!(
                    "live2d profile: runtime {:.2} ms, upload {:.2} ms per frame",
                    self.profile_cpu / 30.0,
                    self.profile_upload / 30.0,
                );
                self.profile_frames = 0;
                self.profile_cpu = 0.0;
                self.profile_upload = 0.0;
            }
        }

        Ok(())
    }

    /// Keeps the model fitted when the viewport aspect ratio changes.
    pub fn resize(&mut self, queue: &Queue, aspect: f32) {
        if !aspect.is_finite() || aspect <= 0.0 {
            return;
        }

        if (self.fit_aspect - aspect).abs() < 1e-4 {
            return;
        }

        self.fit_aspect = aspect;
        let matrix = self.bounds.fit_matrix(aspect, self.view_fill);
        self.gpu.transform.update_matrix(queue, &matrix);
    }

    /// Makes the model look towards a point given in normalized widget space
    /// (`-1.0..=1.0` on both axes, `-1` = left/top).
    pub fn set_gaze(&mut self, gaze: [f32; 2]) {
        let x = gaze[0].clamp(-1.0, 1.0);
        let y = gaze[1].clamp(-1.0, 1.0);

        let runtime = &mut self.model;

        // Mirrors the Cubism Viewer's default mouse-tracking bindings for this
        // rig exactly:
        //
        //   AngleX          <- mouse X
        //   AngleY          <- mouse Y
        //   ParamBodyAngleX <- mouse X
        //   ParamEyeBallX   <- mouse X
        //   ParamEyeBallY   <- mouse Y
        //
        // and nothing else. Adding extra inputs (AngleZ, PositionX/Y, LeanInput)
        // makes our output diverge from the reference renderer, so resist the
        // urge to improve the tracking here.
        match self.gaze_inputs {
            GazeInputs::Tracking => {
                set_tracking_input(runtime, "AngleX", x);
                set_tracking_input(runtime, "AngleY", y);
                set_tracking_input(runtime, "ParamBodyAngleX", x);
            }
            GazeInputs::Model => {
                set_tracking_input(runtime, "ParamAngleX", x);
                set_tracking_input(runtime, "ParamAngleY", y);
                set_tracking_input(runtime, "ParamBodyAngleX", x);
            }
        }

        set_tracking_input(runtime, "ParamEyeBallX", x);
        set_tracking_input(runtime, "ParamEyeBallY", y);

        self.dirty = true;
    }

    /// Records the mask pass and the visible model pass into `encoder`.
    ///
    /// `clip` must be the area of `target` the host allows us to touch; it is
    /// used as both viewport and scissor for the visible pass.
    pub fn encode(
        &self,
        encoder: &mut CommandEncoder,
        target: &TextureView,
        clip: ClipRect,
    ) -> Result<()> {
        if clip.is_empty() {
            return Ok(());
        }

        // Masks are drawn into the shared off-screen buffer with the full
        // viewport: the Live2D mask shader works in the same clip space as the
        // visible pass, so the buffer must keep its own full extent.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("live2d.mask_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.gpu.mask_target.view(),
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

            self.renderer.draw_masks_with_textures(
                &mut pass,
                &self.gpu.mesh_buffers,
                &self.gpu.clipping_resources,
                &self.gpu.textures,
            )?;
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

            self.renderer.draw_with_textures_clipping_and_transform(
                &mut pass,
                &self.gpu.mesh_buffers,
                &self.gpu.textures,
                &self.gpu.clipping_resources,
                &self.gpu.mask_target,
                &self.gpu.transform,
            )?;
        }

        Ok(())
    }

    /// Names of every motion group declared by the model, in model order.
    pub fn motion_groups(&self) -> impl Iterator<Item = &str> {
        self.motion_groups.keys().map(String::as_str)
    }

    /// Number of motion files the model ships with.
    pub fn motion_count(&self) -> usize {
        self.motion_groups.values().map(Vec::len).sum()
    }

    pub fn expression_count(&self) -> usize {
        self.expression_paths.len()
    }

    /// The ordered motion list `play_next_motion` cycles through.
    fn preferred_motions(&self) -> &[PathBuf] {
        self.motion_groups
            .get(IDLE_GROUP)
            .or_else(|| self.motion_groups.values().next())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// File names of the motions in playback order.
    pub fn motion_labels(&self) -> Vec<String> {
        self.preferred_motions()
            .iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect()
    }

    /// Starts a specific motion from the playback list.
    pub fn play_motion(&mut self, index: usize) {
        let Some(path) = self.preferred_motions().get(index).cloned() else {
            return;
        };

        if let Ok(motion) = load_motion(&path) {
            self.player = Some(MotionPlayer::new(motion));
            self.motion_cursor = index.wrapping_add(1);
            self.dirty = true;
        }
    }

    /// Writes a Cubism-style `-1.0..=1.0` offset into a parameter directly.
    ///
    /// Diagnostics only: it lets a test drive an intermediate parameter and tell
    /// "physics chaining is broken" apart from "the input never arrives".
    pub fn set_parameter_offset(&mut self, id: &str, offset: f32) {
        set_tracking_input(&mut self.model, id, offset);
        self.dirty = true;
    }

    /// Reads a parameter value. Diagnostics and tests only.
    pub fn parameter_value(&self, id: &str) -> Option<f32> {
        self.model.parameter_value(id)
    }

    /// Enables or disables physics evaluation (diagnostics / still frames).
    pub fn set_physics_enabled(&mut self, enabled: bool) {
        self.physics_enabled = enabled;
        self.dirty = true;
    }

    /// Stops the current motion so the model shows its rest pose.
    pub fn stop_motion(&mut self) {
        self.player = None;
        self.dirty = true;
    }

    /// Drops the expressions currently applied.
    pub fn clear_expressions(&mut self) {
        self.expressions.stop_all();
        self.dirty = true;
    }

    /// Rebuilds clipping resources unconditionally.
    ///
    /// Diagnostic hook: comparing a run that calls this every frame against one
    /// that does not isolates "stale mask layout" from "wrong mask math".
    pub fn force_clipping_rebuild(&mut self, device: &Device, queue: &Queue) -> Result<()> {
        self.rebuild_clipping(device, queue)
    }

    /// Starts the next motion, preferring the model's idle group and falling
    /// back to cycling every motion in the model.
    pub fn play_next_motion(&mut self) {
        let candidates = self
            .motion_groups
            .get(IDLE_GROUP)
            .or_else(|| self.motion_groups.values().next());

        let Some(candidates) = candidates else {
            return;
        };

        if candidates.is_empty() {
            return;
        }

        let index = self.motion_cursor % candidates.len();
        self.motion_cursor = self.motion_cursor.wrapping_add(1);

        let path = candidates[index].clone();
        match load_motion(&path) {
            Ok(motion) => {
                self.player = Some(MotionPlayer::new(motion));
                self.dirty = true;
            }
            Err(_) => {
                // A broken motion must not kill playback of the whole model.
                self.player = None;
            }
        }
    }

    /// Plays the expression at `index`, if the model has one.
    pub fn play_expression(&mut self, index: usize) {
        let Some(path) = self.expression_paths.get(index).cloned() else {
            return;
        };

        if let Ok(expression) = load_expression(&path) {
            self.expressions.play(expression);
            self.dirty = true;
        }
    }

    /// Plays the next expression in model order, wrapping around.
    pub fn play_next_expression(&mut self) {
        if self.expression_paths.is_empty() {
            return;
        }

        let index = self.expression_cursor % self.expression_paths.len();
        self.expression_cursor = self.expression_cursor.wrapping_add(1);
        self.play_expression(index);
    }

    pub fn expression_names(&self) -> Vec<String> {
        self.expression_paths
            .iter()
            .filter_map(|path| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .collect()
    }

    fn upload(&mut self, device: &Device, queue: &Queue) -> Result<()> {
        let needs_reclip = {
            let meshes = self.model.meshes();

            match self.gpu.mesh_buffers.update_drawables(queue, meshes) {
                Ok(update) => update.bounds_changed() || update.visibility_changed(),
                // A model whose drawable count changed (expression switching
                // art meshes) needs a full rebuild.
                Err(_) => {
                    self.gpu.mesh_buffers = WgpuMeshBuffers::from_drawables(device, meshes)
                        .ok_or(Error::MeshBuffers)?;
                    true
                }
            }
        };

        if needs_reclip {
            self.rebuild_clipping(device, queue)?;
        }

        Ok(())
    }

    fn rebuild_clipping(&mut self, device: &Device, queue: &Queue) -> Result<()> {
        let GpuState {
            mesh_buffers,
            clipping_resources,
            ..
        } = &mut self.gpu;

        let mut plan = WgpuClippingPlan::from_mesh_buffers(mesh_buffers);
        plan.prepare_single_texture_masks(mesh_buffers)?;

        if !self
            .renderer
            .update_clipping_resources(queue, clipping_resources, &plan)?
        {
            *clipping_resources = self.renderer.create_clipping_resources(device, &plan)?;
        }

        Ok(())
    }
}

/// Integer box-filter factor derived from a `0..1` texture scale.
fn downscale_factor(scale: f32) -> u32 {
    if !scale.is_finite() || scale >= 0.999 {
        return 1;
    }

    ((1.0 / scale).round() as u32).max(1)
}

/// Box-filtered integer downscale.
///
/// Averaging happens in premultiplied space and is then un-premultiplied, so
/// texels that neighbour fully transparent pixels do not pick up dark halos.
fn downscale_rgba(width: u32, height: u32, rgba: &[u8], factor: u32) -> (u32, u32, Vec<u8>) {
    let factor = factor.max(1);

    if factor == 1 {
        return (width, height, rgba.to_vec());
    }

    let out_width = (width / factor).max(1);
    let out_height = (height / factor).max(1);
    let mut out = vec![0u8; (out_width * out_height * 4) as usize];

    for out_y in 0..out_height {
        for out_x in 0..out_width {
            let mut acc = [0.0_f32; 4];
            let mut samples = 0.0_f32;

            for dy in 0..factor {
                for dx in 0..factor {
                    let x = out_x * factor + dx;
                    let y = out_y * factor + dy;

                    if x >= width || y >= height {
                        continue;
                    }

                    let index = ((y * width + x) * 4) as usize;
                    let alpha = f32::from(rgba[index + 3]) / 255.0;

                    acc[0] += f32::from(rgba[index]) * alpha;
                    acc[1] += f32::from(rgba[index + 1]) * alpha;
                    acc[2] += f32::from(rgba[index + 2]) * alpha;
                    acc[3] += alpha;
                    samples += 1.0;
                }
            }

            if samples == 0.0 || acc[3] <= f32::EPSILON {
                continue;
            }

            let index = ((out_y * out_width + out_x) * 4) as usize;
            let alpha = acc[3] / samples;

            out[index] = (acc[0] / acc[3]).round().clamp(0.0, 255.0) as u8;
            out[index + 1] = (acc[1] / acc[3]).round().clamp(0.0, 255.0) as u8;
            out[index + 2] = (acc[2] / acc[3]).round().clamp(0.0, 255.0) as u8;
            out[index + 3] = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    (out_width, out_height, out)
}

fn motion_paths_by_group(

    runtime: &mocari::ModelRuntime,
    model_dir: Option<&Path>,
) -> BTreeMap<String, Vec<PathBuf>> {
    let Some(model_dir) = model_dir else {
        return BTreeMap::new();
    };

    runtime
        .model()
        .motions()
        .iter()
        .map(|(group, references)| {
            (
                group.clone(),
                references
                    .iter()
                    .map(|reference| model_dir.join(reference.file()))
                    .collect(),
            )
        })
        .collect()
}

fn expression_paths(
    runtime: &mocari::ModelRuntime,
    model_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let Some(model_dir) = model_dir else {
        return Vec::new();
    };

    runtime
        .model()
        .expressions()
        .iter()
        .map(|reference| model_dir.join(reference.file()))
        .collect()
}

struct ScannedSidecars {
    motions: Vec<PathBuf>,
    expressions: Vec<PathBuf>,
}

/// Walks the model directory looking for `.motion3.json` / `.exp3.json` files.
fn scan_sidecars(dir: &Path) -> ScannedSidecars {
    let mut motions = Vec::new();
    let mut expressions = Vec::new();
    collect_sidecars(dir, 0, &mut motions, &mut expressions);

    motions.sort();
    // Stable sort keyed on "not an idle clip", so an `IDLE.motion3.json` style
    // file plays first while everything else keeps alphabetical order.
    motions.sort_by_key(|path| !is_idle_clip(path));
    expressions.sort();

    ScannedSidecars {
        motions,
        expressions,
    }
}

fn collect_sidecars(
    dir: &Path,
    depth: usize,
    motions: &mut Vec<PathBuf>,
    expressions: &mut Vec<PathBuf>,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            collect_sidecars(&path, depth + 1, motions, expressions);
            continue;
        }

        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        let lower = name.to_ascii_lowercase();

        if lower.ends_with(".motion3.json") {
            motions.push(path);
        } else if lower.ends_with(".exp3.json") {
            expressions.push(path);
        }
    }
}

fn is_idle_clip(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase().contains("idle"))
        .unwrap_or(false)
}

/// Writes a Cubism-style `-1.0..=1.0` tracking offset into a parameter.
///
/// mocari's `*_normalized` setters take `0.0..=1.0` (the docs say so) and
/// clamp, so passing a `-1.0..=1.0` value silently pins every negative offset to
/// the parameter minimum: the model freezes until the cursor crosses its centre
/// line and then snaps into motion. Convert here, once.
fn set_tracking_input(runtime: &mut ModelRuntime, id: &str, offset: f32) {
    let amount = (offset.clamp(-1.0, 1.0) + 1.0) * 0.5;
    runtime.set_parameter_override_normalized(id, amount);
}
