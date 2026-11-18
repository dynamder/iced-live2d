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
use mocari::json::Physics3;
use mocari::motion::load_motion;
use mocari::render::wgpu::{
    WgpuClippingPlan, WgpuClippingResources, WgpuLive2dRenderer, WgpuMaskRenderTarget,
    WgpuMeshBuffers, WgpuTexture, WgpuTransform,
};
use mocari::{ExpressionManager, ModelRuntime, MotionPlayer};
use wgpu::{CommandEncoder, Device, Queue, TextureFormat, TextureView};

use crate::bounds::ModelBounds;
use crate::error::{Error, Result};
use crate::rig::{self, ModelFacts, Rig, Slot, TrackingMode};

/// Default size of the single shared mask render target.
///
/// Cubism models draw every clipped drawable into this off-screen buffer before
/// the visible pass. Bigger costs VRAM but keeps mask edges crisp on 4096px
/// textures.
const MASK_TARGET_SIZE: u32 = 1024;

const DEFAULT_VIEW_FILL: f32 = 1.85;
/// Animation step the scene advances in, regardless of the host's frame rate.
///
/// 60 Hz is what Live2D models are authored against: motion curves and physics
/// are sampled on it, and two hosts with the same elapsed time must agree on the
/// resulting pose.
///
/// A host that wants exactly one step per frame should pass this as the interval
/// to [`Live2dScene::advance`]. Passing a rounded interval instead (16 ms, say)
/// is fine but slowly drifts below real time, because the scene only spends whole
/// steps.
pub const STEP: Duration = Duration::from_micros(16_667);
/// Longest interval one `advance` call may count, so a stalled window does not
/// teleport the motion timeline.
const MAX_CALL: Duration = Duration::from_millis(100);
/// Time the scene may bank and spend on steps, i.e. how far it may fall behind
/// before it starts dropping time instead of catching up.
const MAX_CARRY: Duration = STEP.saturating_mul(4);
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
    /// Where the cursor writes its angles. [`TrackingMode::Adapted`] follows the
    /// rig's own conventions; [`TrackingMode::ViewerDefault`] reproduces the
    /// Cubism Viewer's mouse bindings, which on a physics rig means only the eyes
    /// respond.
    pub tracking: TrackingMode,
}

impl Default for SceneOptions {
    fn default() -> Self {
        Self {
            texture_scale: 0.5,
            view_fill: DEFAULT_VIEW_FILL,
            mask_size: MASK_TARGET_SIZE,
            tracking: TrackingMode::default(),
        }
    }
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
    /// swap) so the next step rebuilds meshes even if no motion is playing.
    dirty: bool,
    stepper: Stepper,
    gpu: GpuState,
    bounds: ModelBounds,
    view_fill: f32,
    fit_aspect: f32,
    motion_cursor: usize,
    /// Where the cursor writes, resolved once at load time by [`crate::rig`].
    rig: Rig,
    physics_enabled: bool,
    profile: Option<Profile>,
    expression_cursor: usize,
    issues: Vec<Error>,
    /// Last error that repeats every frame, so it is queued once, not 60 times a
    /// second.
    repeating_issue: Option<String>,
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
        Self::load_with_options(
            device,
            queue,
            model_path,
            color_format,
            SceneOptions::default(),
        )
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
                    downscale_rgba(texture.width(), texture.height(), texture.rgba(), factor)
                } else {
                    (texture.width(), texture.height(), texture.rgba().to_vec())
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

        let mesh_buffers =
            WgpuMeshBuffers::from_drawables(device, model.meshes()).ok_or(Error::MeshBuffers)?;

        let clipping_resources = {
            let mut plan = WgpuClippingPlan::from_mesh_buffers(&mesh_buffers);
            plan.prepare_single_texture_masks(&mesh_buffers)?;
            renderer.create_clipping_resources(device, &plan)?
        };

        let mask_target = renderer.create_mask_render_target(device, options.mask_size.max(64))?;
        let transform =
            renderer.create_transform(device, &bounds.fit_matrix(1.0, options.view_fill));

        // Rigs exported for VTuber software drive tracking through AngleX/Y/Z,
        // PositionX/Y/Z and LeanInput; physics then derives ParamAngle*,
        // ParamBodyAngle* and the rest. The resolver in `crate::rig` works out
        // which of the two conventions a model uses, from its physics wiring.

        let mut motion_groups = motion_paths_by_group(&model, model_dir.as_deref());
        let mut expression_paths = expression_paths(&model, model_dir.as_deref());

        // Everything below only needs the GPU copies.
        drop(loaded);

        // Plenty of exported models (VTuber rigs in particular) ship their
        // motion and expression files next to the model without listing them in
        // `model3.json`. Fall back to scanning the model directory.
        let mut issues = Vec::new();

        let physics_outputs = physics_output_parameters(&model, model_dir.as_deref(), &mut issues);
        let rig = rig::resolve(
            options.tracking,
            &ModelFacts::new(model.parameter_ids().to_vec(), physics_outputs),
        );

        if let Some(model_dir) = model_dir.as_deref()
            && (motion_groups.is_empty() || expression_paths.is_empty())
        {
            let scanned = scan_sidecars(model_dir, &mut issues);

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
            stepper: Stepper::new(),
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
            rig,
            physics_enabled: true,
            profile: std::env::var_os("LIVE2D_PROFILE").map(|_| Profile::default()),
            expression_cursor: 0,
            issues,
            repeating_issue: None,
        };

        scene.play_next_motion();
        Ok(scene)
    }

    /// Advances the animation by `delta` and uploads any changed geometry.
    ///
    /// The host may pass whatever interval it measured; the scene consumes it in
    /// fixed [`STEP`] slices and keeps the remainder for the next call, so the
    /// pose depends on how much time passed, not on how the host happened to
    /// slice it. Two consequences worth knowing:
    ///
    /// - A zero-length `delta` is a no-op. It must be: a frame that does not
    ///   advance time would otherwise re-evaluate the model with `delta = 0`,
    ///   which resets every parameter and then skips the physics write, dropping
    ///   the physics-driven parts into their default pose for exactly one frame.
    ///   That reads as a flicker at whatever rate the host produces such frames.
    /// - Time beyond [`MAX_CARRY`] is dropped rather than replayed, so a stalled
    ///   window does not freeze the next few frames while it catches up.
    pub fn advance(&mut self, device: &Device, queue: &Queue, delta: Duration) -> Result<()> {
        let steps = self.stepper.consume(delta);

        let animating = self.dirty
            || self.player.is_some()
            || self.expressions.active_expression_count() > 0
            || self.model.physics().is_some();

        if !animating {
            return Ok(());
        }

        let started = std::time::Instant::now();

        for _ in 0..steps {
            self.simulate();
        }

        // Deforming the mesh is a pure function of the final parameter state, so
        // a call that owes several steps still only deforms once.
        let cost = (steps > 0)
            .then(|| self.rebuild_geometry(device, queue, started))
            .transpose()?;

        if let Some(profile) = self.profile.as_mut()
            && let Some(line) = profile.record(delta, steps, cost)
        {
            println!("{line}");
        }

        Ok(())
    }

    /// One fixed-length animation step: replay motion, expression, overrides,
    /// physics and pose, leaving the parameters where the last step of this frame
    /// renders them.
    fn simulate(&mut self) {
        let seconds = STEP.as_secs_f32();
        let mut finished_motion = false;

        let runtime = &mut self.model;

        runtime.reset_parameters();
        runtime.reset_part_opacities();

        if let Some(player) = self.player.as_mut() {
            player.tick(seconds);
            player.apply(runtime);
            finished_motion = player.is_finished();
        }

        self.expressions.tick(seconds);
        self.expressions.apply(runtime);

        // Overrides (gaze, user sliders) win over motion and expression.
        runtime.apply_parameter_overrides();

        if self.physics_enabled {
            runtime.apply_physics(seconds);
        }
        runtime.apply_pose(seconds);

        if finished_motion {
            self.player = None;
            self.play_next_motion();
        }
    }

    /// Deforms every drawable from the current parameters and pushes the result
    /// to the GPU.
    fn rebuild_geometry(
        &mut self,
        device: &Device,
        queue: &Queue,
        started: std::time::Instant,
    ) -> Result<GeometryCost> {
        if self.model.update_meshes().is_none() {
            return Err(Error::MeshUpdate);
        }

        self.dirty = false;

        let cpu_ms = started.elapsed().as_secs_f64() * 1000.0;
        let upload = self.upload(device, queue)?;

        Ok(GeometryCost { cpu_ms, upload })
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
    ///
    /// Which parameters that means is decided at load time by [`crate::rig`]: a
    /// rig authored for VTube Studio takes head rotation on `AngleX`/`AngleY` and
    /// body motion on `PositionX`/`PositionY`, and physics derives
    /// `ParamAngle*`/`ParamBodyAngle*` from them. Under
    /// [`TrackingMode::ViewerDefault`] the conventional `ParamAngle*` names are
    /// written instead, which on such a rig only moves the eyes — the same thing
    /// the Cubism Viewer does with its default bindings.
    pub fn set_gaze(&mut self, gaze: [f32; 2]) {
        let x = gaze[0].clamp(-1.0, 1.0);
        let y = gaze[1].clamp(-1.0, 1.0);

        let Self {
            model, rig, dirty, ..
        } = self;

        for (slot, value) in [
            (Slot::HeadYaw, x),
            (Slot::HeadPitch, y),
            (Slot::BodyYaw, x),
            (Slot::BodyLean, y),
            (Slot::EyeX, x),
            (Slot::EyeY, y),
        ] {
            if let Some(binding) = rig.binding(slot) {
                set_tracking_input(model, &binding.parameter, value);
            }
        }

        *dirty = true;
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

    /// Starts a specific motion from the playback list.
    pub fn play_motion(&mut self, index: usize) {
        let (count, path) = {
            let candidates = self.preferred_motions();
            (candidates.len(), candidates.get(index).cloned())
        };

        let Some(path) = path else {
            self.issues.push(Error::MotionIndex { index, count });
            return;
        };

        self.motion_cursor = index.wrapping_add(1);

        match load_motion(&path) {
            Ok(motion) => {
                self.player = Some(MotionPlayer::new(motion));
                self.dirty = true;
            }
            Err(source) => self.issues.push(Error::Motion { path, source }),
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

    /// Every parameter id, in model order. Diagnostics and tests only.
    pub fn parameter_ids(&self) -> &[String] {
        self.model.parameter_ids()
    }

    /// Where the cursor writes, as resolved for this model at load time.
    pub fn rig(&self) -> &Rig {
        &self.rig
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
        let count = self.preferred_motions().len();

        if count == 0 {
            return;
        }

        let index = self.motion_cursor % count;
        self.play_motion(index);
    }

    /// Plays the expression at `index`, if the model has one.
    pub fn play_expression(&mut self, index: usize) {
        let count = self.expression_paths.len();

        let Some(path) = self.expression_paths.get(index).cloned() else {
            self.issues.push(Error::ExpressionIndex { index, count });
            return;
        };

        self.expression_cursor = index.wrapping_add(1);

        match load_expression(&path) {
            Ok(expression) => {
                self.expressions.play(expression);
                self.dirty = true;
            }
            Err(source) => self.issues.push(Error::Expression { path, source }),
        }
    }

    /// Plays the next expression in model order, wrapping around.
    pub fn play_next_expression(&mut self) {
        let count = self.expression_paths.len();

        if count == 0 {
            return;
        }

        let index = self.expression_cursor % count;
        self.play_expression(index);
    }

    /// Queues an error at most once per distinct message.
    ///
    /// The upload path can fail on every frame of a model that keeps changing
    /// shape; pushing that into `issues` each time would bury everything else.
    fn report_once(&mut self, error: Error) {
        let message = error.to_string();

        if self.repeating_issue.as_deref() == Some(message.as_str()) {
            return;
        }

        self.repeating_issue = Some(message);
        self.issues.push(error);
    }

    /// Drains the non-fatal problems collected since the last call.
    ///
    /// A broken sidecar file, an out-of-range index or an unreadable animation
    /// directory never aborts playback, so they queue up here instead of being
    /// dropped. Drain and print them: silence is otherwise indistinguishable
    /// from "this model has no motions".
    pub fn take_issues(&mut self) -> Vec<Error> {
        std::mem::take(&mut self.issues)
    }

    fn upload(&mut self, device: &Device, queue: &Queue) -> Result<UploadCost> {
        let mesh_start = std::time::Instant::now();

        let mut rejected = None;

        let (needs_reclip, drawables) = {
            let meshes = self.model.meshes();

            match self.gpu.mesh_buffers.update_drawables(queue, meshes) {
                Ok(update) => (
                    update.bounds_changed()
                        || update.visibility_changed()
                        || update.structure_changed(),
                    update.uploaded_drawables(),
                ),
                // A drawable changed shape in a way the buffers cannot absorb, so
                // the whole set is rebuilt. That is correct but expensive, and a
                // model doing it every frame looks exactly like a slow one.
                Err(error) => {
                    self.gpu.mesh_buffers = WgpuMeshBuffers::from_drawables(device, meshes)
                        .ok_or(Error::MeshBuffers)?;
                    rejected = Some(Error::from(error));
                    (true, meshes.len())
                }
            }
        };

        if let Some(error) = rejected {
            self.report_once(error);
        }

        let mesh_ms = mesh_start.elapsed().as_secs_f64() * 1000.0;
        let clip_start = std::time::Instant::now();

        if needs_reclip {
            self.rebuild_clipping(device, queue)?;
        }

        Ok(UploadCost {
            mesh_ms,
            clip_ms: clip_start.elapsed().as_secs_f64() * 1000.0,
            drawables,
        })
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

/// Fixed-timestep accumulator between the host's frame pacing and the scene.
///
/// Hosts hand the scene whatever interval they measured, which on a live window
/// is neither constant nor never zero. Consuming it in whole [`STEP`]s keeps the
/// pose a function of elapsed time alone: the same input sequence yields the same
/// pose no matter how the frames were sliced.
#[derive(Debug, Clone)]
struct Stepper {
    carry: Duration,
}

impl Stepper {
    /// Starts one step in debt so the first `advance` renders the loaded model
    /// instead of waiting a frame for time to accumulate.
    fn new() -> Self {
        Self { carry: STEP }
    }

    /// Takes `delta`, returns how many steps are due now.
    ///
    /// Time beyond [`MAX_CARRY`] is dropped rather than banked: a window that
    /// stalled for a second must not make the model sprint to catch up.
    fn consume(&mut self, delta: Duration) -> u32 {
        self.carry = (self.carry + delta.clamp(Duration::ZERO, MAX_CALL)).min(MAX_CARRY);

        let steps = (self.carry.as_nanos() / STEP.as_nanos()) as u32;
        self.carry -= STEP.saturating_mul(steps);

        steps
    }
}

/// CPU and upload cost of one geometry rebuild, in milliseconds.
#[derive(Debug, Clone, Copy, Default)]
struct GeometryCost {
    cpu_ms: f64,
    upload: UploadCost,
}

/// Where the upload time went: vertex/index buffers, or clipping resources.
#[derive(Debug, Clone, Copy, Default)]
struct UploadCost {
    mesh_ms: f64,
    clip_ms: f64,
    drawables: usize,
}

/// Per-call cost and frame-interval statistics, printed once every
/// [`Profile::REPORT_EVERY`] calls when `LIVE2D_PROFILE` is set.
///
/// `steps` is the number of animation steps those calls ran, so `steps / frames`
/// well above 1 means the host presents slower than the scene steps and is
/// paying for catch-up work. Cost is reported per frame (per `advance` call),
/// which is what a frame budget cares about.
struct Profile {
    frames: u32,
    steps: u32,
    cpu_ms: f64,
    mesh_ms: f64,
    clip_ms: f64,
    drawables: usize,
    delta_ms: f64,
    delta_min: Duration,
    delta_max: Duration,
    zero_frames: u32,
    short_frames: u32,
}

impl Profile {
    const REPORT_EVERY: u32 = 60;
    /// At or below this interval a frame advances the scene by almost nothing.
    const SHORT_FRAME: Duration = Duration::from_millis(1);

    fn record(
        &mut self,
        delta: Duration,
        steps: u32,
        cost: Option<GeometryCost>,
    ) -> Option<String> {
        self.frames += 1;
        self.steps += steps;

        if let Some(cost) = cost {
            self.cpu_ms += cost.cpu_ms;
            self.mesh_ms += cost.upload.mesh_ms;
            self.clip_ms += cost.upload.clip_ms;
            self.drawables += cost.upload.drawables;
        }

        self.delta_ms += delta.as_secs_f64() * 1000.0;
        self.delta_min = self.delta_min.min(delta);
        self.delta_max = self.delta_max.max(delta);

        if delta.is_zero() {
            self.zero_frames += 1;
        } else if delta <= Self::SHORT_FRAME {
            self.short_frames += 1;
        }

        if self.frames < Self::REPORT_EVERY {
            return None;
        }

        let count = f64::from(self.frames);
        let line = format!(
            "live2d profile: {} frames {:.2} steps, rebuild {:.2} ms, upload {:.2} ms (mesh {:.2} over {:.0} drawables, clip {:.2}), delta {:.2}..{:.1} ms (mean {:.2}, {} zero, {} under 1 ms)",
            self.frames,
            f64::from(self.steps) / count,
            self.cpu_ms / count,
            (self.mesh_ms + self.clip_ms) / count,
            self.mesh_ms / count,
            self.drawables as f64 / count,
            self.clip_ms / count,
            self.delta_min.as_secs_f64() * 1000.0,
            self.delta_max.as_secs_f64() * 1000.0,
            self.delta_ms / count,
            self.zero_frames,
            self.short_frames,
        );

        *self = Self::default();

        Some(line)
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            frames: 0,
            steps: 0,
            cpu_ms: 0.0,
            mesh_ms: 0.0,
            clip_ms: 0.0,
            drawables: 0,
            delta_ms: 0.0,
            delta_min: Duration::MAX,
            delta_max: Duration::ZERO,
            zero_frames: 0,
            short_frames: 0,
        }
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

fn expression_paths(runtime: &mocari::ModelRuntime, model_dir: Option<&Path>) -> Vec<PathBuf> {
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

/// Every parameter a physics setting writes, i.e. the ones a host must not drive.
///
/// Read from the `physics3.json` the model references. A missing or unparsable
/// file yields an empty set — the resolver then falls back to the conventional
/// names — but it is reported rather than silently ignored.
fn physics_output_parameters(
    model: &ModelRuntime,
    model_dir: Option<&Path>,
    issues: &mut Vec<Error>,
) -> Vec<String> {
    let (Some(dir), Some(file)) = (model_dir, model.model().physics()) else {
        return Vec::new();
    };

    let path = dir.join(file);

    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(source) => {
            issues.push(Error::PhysicsFile { path, source });
            return Vec::new();
        }
    };

    let physics = match Physics3::from_json_str(&source) {
        Ok(physics) => physics,
        Err(_) => {
            issues.push(Error::PhysicsParse { path });
            return Vec::new();
        }
    };

    physics
        .settings()
        .iter()
        .flat_map(|setting| setting.outputs())
        .map(|output| output.destination().id().to_owned())
        .collect()
}

/// Walks the model directory looking for `.motion3.json` / `.exp3.json` files.
///
/// Unreadable directories are pushed into `issues` rather than skipped: a scan
/// that silently finds nothing looks exactly like a model with no animations.
fn scan_sidecars(dir: &Path, issues: &mut Vec<Error>) -> ScannedSidecars {
    let mut motions = Vec::new();
    let mut expressions = Vec::new();
    collect_sidecars(dir, 0, &mut motions, &mut expressions, issues);

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
    issues: &mut Vec<Error>,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(source) => {
            issues.push(Error::Scan {
                path: dir.to_path_buf(),
                source,
            });
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            collect_sidecars(&path, depth + 1, motions, expressions, issues);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Most steps a `Stepper` can hand out across `calls` identical `delta`s.
    fn steps_over(deltas: &[Duration]) -> u32 {
        let mut stepper = Stepper::new();
        deltas.iter().map(|delta| stepper.consume(*delta)).sum()
    }

    #[test]
    fn the_first_call_steps_the_loaded_model() {
        assert_eq!(steps_over(&[Duration::ZERO]), 1);
    }

    #[test]
    fn zero_length_frames_never_step() {
        let mut stepper = Stepper::new();
        stepper.consume(Duration::ZERO);

        for _ in 0..10 {
            assert_eq!(stepper.consume(Duration::ZERO), 0);
        }
    }

    #[test]
    fn one_frame_interval_is_one_step() {
        let mut stepper = Stepper::new();
        stepper.consume(Duration::ZERO);

        assert_eq!(stepper.consume(STEP), 1);
        assert_eq!(stepper.consume(STEP), 1);
    }

    #[test]
    fn leftover_time_carries_over() {
        let half = STEP / 2;
        let mut stepper = Stepper::new();
        stepper.consume(Duration::ZERO);

        assert_eq!(stepper.consume(half), 0);
        assert_eq!(stepper.consume(half), 1);
    }

    #[test]
    fn a_slow_host_runs_several_steps_per_call() {
        assert_eq!(steps_over(&[Duration::ZERO, STEP * 2]), 3);
    }

    #[test]
    fn steps_follow_elapsed_time_not_frame_count() {
        let quarter = STEP / 4;
        let chunked = steps_over(&[quarter; 8]);
        let whole = steps_over(&[quarter * 8]);

        assert_eq!(chunked, whole);
        assert_eq!(whole, 3);
    }

    #[test]
    fn a_stall_does_not_bank_time() {
        let mut stepper = Stepper::new();
        stepper.consume(Duration::ZERO);

        assert_eq!(stepper.consume(Duration::from_secs(10)), 4);
        assert_eq!(stepper.consume(Duration::ZERO), 0);
        assert_eq!(stepper.consume(Duration::ZERO), 0);
    }
}
