//! The render layer's side of the component: three tables and one primitive.
//!
//! iced creates exactly one pipeline per primitive type, so this is where all the
//! per-asset, per-model and per-view state lives, keyed by identity:
//!
//! - `assets` — parsed and uploaded once, shared by every model of the asset;
//! - `models` — one instance each, with its own GPU mirror and event listeners;
//! - `views` — one framing each, so two widgets can show one model at different
//!   sizes.
//!
//! A slot is reclaimed when the host drops what it stands for: every slot holds a
//! `Weak` to a `Presence` the host object owns.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use iced::Rectangle;
use iced::widget::shader;
use iced::wgpu;
use live2d_render::{
    Asset, AssetId, AssetOptions, ClipRect, Error, IDLE_GROUP, LookAtMode, Model, ModelGpu,
    ModelInfo, ModelOptions, Prepared, View, WgpuLive2dRenderer, declared_idle,
};

use crate::event::{LoadFailure, Problem};
use crate::interface::{InterfaceSpec, KeyId, Mapping, VocabularySpec};
use crate::model::{ModelId, PointerInput, Presence};

/// Source of view identities, one per widget slot in the UI tree.
static NEXT_VIEW: AtomicU64 = AtomicU64::new(1);

/// Identity of one widget's framing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ViewId(u64);

impl ViewId {
    pub(crate) fn unique() -> Self {
        Self(NEXT_VIEW.fetch_add(1, Ordering::Relaxed))
    }
}

/// A pointer interaction, as the widget saw it.
#[derive(Debug, Clone)]
pub(crate) enum PointerEventKind {
    Pressed { area: Option<KeyId>, at: [f32; 2] },
    Released { area: Option<KeyId>, at: [f32; 2] },
    Entered { area: KeyId },
    Exited { area: KeyId },
    Dragged { delta: [f32; 2] },
    Scrolled { lines: f32 },
}

/// Something the pipeline tells the widgets that draw a model.
///
/// Non-generic on purpose: the pipeline never sees a host type, only the plain
/// data an interface resolves to. The widget turns [`KeyId`]s back into typed
/// keys.
#[derive(Debug, Clone)]
pub(crate) enum PipelineEvent {
    Loaded {
        info: Arc<ModelInfo>,
    },
    LoadFailed {
        failure: LoadFailure,
    },
    Problem(Problem),
    MotionStarted {
        group: KeyId,
        clip: usize,
        duration: f32,
        looping: bool,
    },
    MotionFinished {
        group: KeyId,
        clip: usize,
    },
    Pointer(PointerEventKind),
}

/// Something the host asked the model to do, already reduced to vocabulary
/// indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrimitiveCommand {
    PlayMotion { group: usize, clip: usize },
    PlayRandomMotion { group: usize },
    PlayExpression { expression: usize },
    StopMotion,
    ClearExpressions,
}

/// What plays when nothing else does, already reduced to a vocabulary index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleInput {
    Declared,
    Group(usize),
    Off,
}

/// One frame of one model's inputs.
///
/// Rebuilt by the widget on every frame and dropped after it is rendered, so it
/// carries plain data only — no GPU state, no `Rc`, nothing that would make the
/// render path need a lock.
#[derive(Debug)]
pub struct Live2dPrimitive {
    pub(crate) asset: AssetId,
    pub(crate) asset_options: AssetOptions,
    pub(crate) path: Arc<PathBuf>,
    pub(crate) model: ModelId,
    pub(crate) model_options: ModelOptions,
    pub(crate) view: ViewId,
    pub(crate) spec: InterfaceSpec,
    pub(crate) fill: f32,
    pub(crate) inputs: FrameInputs,
    pub(crate) events: Option<Sender<PipelineEvent>>,
    pub(crate) asset_presence: Arc<Presence>,
    pub(crate) model_presence: Arc<Presence>,
    pub(crate) view_presence: Arc<Presence>,
}

#[derive(Debug)]
pub(crate) struct FrameInputs {
    pub(crate) parameters: Vec<(usize, f32)>,
    pub(crate) parts: Vec<(usize, f32)>,
    pub(crate) commands: Vec<PrimitiveCommand>,
    pub(crate) problems: Vec<Problem>,
    pub(crate) look_at: Option<[f32; 2]>,
    pub(crate) look_at_mode: LookAtMode,
    pub(crate) physics_enabled: bool,
    pub(crate) idle: IdleInput,
    pub(crate) paused: bool,
    /// Where the cursor is, in clip space (`-1.0..=1.0`, `+y` up).
    pub(crate) cursor: Option<[f32; 2]>,
    pub(crate) pointer: Vec<PointerInput>,
}

impl shader::Primitive for Live2dPrimitive {
    type Pipeline = Live2dPipeline;

    fn prepare(
        &self,
        pipeline: &mut Live2dPipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        _viewport: &shader::Viewport,
    ) {
        pipeline.prepare(self, device, queue, bounds);
    }

    fn render(
        &self,
        pipeline: &Live2dPipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        pipeline.render(self, encoder, target, clip_bounds);
    }
}

/// Per-model GPU state and bookkeeping, shared by every widget that draws it.
pub struct Live2dPipeline {
    renderer: WgpuLive2dRenderer,
    assets: HashMap<AssetId, AssetSlot>,
    models: HashMap<ModelId, ModelSlot>,
    views: HashMap<ViewId, ViewSlot>,
    /// Events found while *rendering*, handed to the next `prepare` so that they
    /// can be broadcast with everything else.
    pending: Mutex<Pending>,
    profile: Option<Profile>,
}

impl shader::Pipeline for Live2dPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self {
            renderer: WgpuLive2dRenderer::new(device, format),
            assets: HashMap::new(),
            models: HashMap::new(),
            views: HashMap::new(),
            pending: Mutex::new(Pending::default()),
            profile: std::env::var_os("LIVE2D_PROFILE").map(|_| Profile::default()),
        }
    }

    fn trim(&mut self) {
        let live: Vec<AssetId> = self
            .models
            .values()
            .filter(|slot| slot.presence.strong_count() > 0)
            .map(|slot| slot.asset)
            .collect();

        self.assets
            .retain(|id, slot| slot.presence.strong_count() > 0 || live.contains(id));
        self.models.retain(|_, slot| slot.presence.strong_count() > 0);
        self.views.retain(|_, slot| slot.presence.strong_count() > 0);

        let models = &self.models;
        lock(&self.pending)
            .render_failures
            .retain(|id, _| models.contains_key(id));
    }
}

impl Live2dPipeline {
    fn prepare(
        &mut self,
        primitive: &Live2dPrimitive,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
    ) {
        self.dispatch_pending();

        let aspect = aspect_of(bounds);
        let Self {
            renderer,
            assets,
            models,
            views,
            profile,
            ..
        } = self;

        let gpu = Gpu {
            renderer,
            device,
            queue,
        };

        if let Entry::Vacant(slot) = models.entry(primitive.model) {
            slot.insert(build_model(primitive, assets, &gpu));
        }

        if let Some(info) = assets.get(&primitive.asset).and_then(AssetSlot::info) {
            let model_bounds = info.bounds;

            let slot = views.entry(primitive.view).or_insert_with(|| ViewSlot {
                view: View::new(renderer, device, model_bounds, aspect, primitive.fill),
                presence: Arc::downgrade(&primitive.view_presence),
            });

            slot.presence = Arc::downgrade(&primitive.view_presence);
            slot.view.set_aspect(queue, aspect);
            slot.view.set_fill(queue, primitive.fill);
        }

        if let Some(slot) = models.get_mut(&primitive.model) {
            register(slot, primitive.events.as_ref());
        }

        let (Some(asset), Some(view)) = (
            assets.get(&primitive.asset),
            views.get(&primitive.view),
        ) else {
            return;
        };

        let AssetState::Ready(asset) = &asset.state else {
            return;
        };

        let Some(slot) = models.get_mut(&primitive.model) else {
            return;
        };

        let ModelSlot {
            listeners, state, ..
        } = slot;

        let ModelState::Ready(ready) = state else {
            return;
        };

        let cost = reconcile(primitive, asset.info(), &view.view, ready, listeners, &gpu);

        if let Some(profile) = profile.as_mut()
            && let Some(line) = profile.record(cost)
        {
            println!("{line}");
        }
    }

    fn render(
        &self,
        primitive: &Live2dPrimitive,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let (Some(AssetSlot { state: AssetState::Ready(asset), .. }), Some(ModelSlot { state: ModelState::Ready(ready), .. }), Some(view)) = (
            self.assets.get(&primitive.asset),
            self.models.get(&primitive.model),
            self.views.get(&primitive.view),
        ) else {
            return;
        };

        let clip = ClipRect {
            x: clip_bounds.x,
            y: clip_bounds.y,
            width: clip_bounds.width,
            height: clip_bounds.height,
        };

        if let Err(error) =
            ready
                .gpu
                .encode(&self.renderer, encoder, target, clip, asset.textures(), &view.view)
        {
            self.push_render_failure(primitive.model, error);
        }
    }

    fn push_render_failure(&self, model: ModelId, error: Error) {
        let message = error.to_string();
        let mut pending = lock(&self.pending);

        if pending.render_failures.get(&model) == Some(&message) {
            return;
        }

        pending.render_failures.insert(model, message);
        pending.events.push((
            model,
            PipelineEvent::Problem(Problem::Renderer {
                error: Arc::new(error),
            }),
        ));
    }

    /// Hands the events `render` could not broadcast to the models they belong to.
    fn dispatch_pending(&mut self) {
        let events = std::mem::take(&mut lock(&self.pending).events);

        for (model, event) in events {
            if let Some(slot) = self.models.get_mut(&model) {
                broadcast(&mut slot.listeners, event);
            }
        }
    }
}

fn lock(pending: &Mutex<Pending>) -> MutexGuard<'_, Pending> {
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct Pending {
    events: Vec<(ModelId, PipelineEvent)>,
    /// Last render failure per model, so a failure that repeats every frame is
    /// reported once.
    render_failures: HashMap<ModelId, String>,
}

struct AssetSlot {
    presence: Weak<Presence>,
    state: AssetState,
}

enum AssetState {
    Parsed(Box<Prepared>),
    Ready(Box<Asset>),
    Failed(Arc<Error>),
}

impl AssetSlot {
    fn info(&self) -> Option<&Arc<ModelInfo>> {
        self.state.info()
    }

    fn from_parse(primitive: &Live2dPrimitive) -> Self {
        let presence = Arc::downgrade(&primitive.asset_presence);

        let state = match Asset::parse(primitive.asset, primitive.path.as_path()) {
            Ok(prepared) => AssetState::Parsed(Box::new(prepared)),
            Err(error) => AssetState::Failed(Arc::new(error)),
        };

        Self { presence, state }
    }

}

impl AssetState {
    fn info(&self) -> Option<&Arc<ModelInfo>> {
        match self {
            AssetState::Parsed(prepared) => Some(prepared.info()),
            AssetState::Ready(asset) => Some(asset.info()),
            AssetState::Failed(_) => None,
        }
    }
}

struct ViewSlot {
    view: View,
    presence: Weak<Presence>,
}

struct ModelSlot {
    presence: Weak<Presence>,
    /// The asset this model draws; the asset slot outlives the models using it.
    asset: AssetId,
    listeners: Vec<Sender<PipelineEvent>>,
    state: ModelState,
}

enum ModelState {
    Ready(Box<Ready>),
    Failed(LoadFailure),
}

struct Ready {
    model: Model,
    gpu: ModelGpu,
    mapping: Mapping,
    /// When this model was last advanced, so time is measured per model.
    last: Option<Instant>,
    /// Last problem reported from the update path, so a repeating failure is not
    /// reported sixty times a second.
    last_problem: Option<String>,
    /// The hit area under the cursor, as a model index.
    hover: Option<usize>,
    /// Whether a button is held down over the widget.
    down: bool,
    /// Where the cursor was, in model space, the last time a drag was reported.
    drag: Option<[f32; 2]>,
    parameters: Vec<(usize, f32)>,
    parts: Vec<(usize, f32)>,
}

/// Parses the asset if needed, then validates this model's interface against it
/// and only then uploads the textures.
fn build_model(
    primitive: &Live2dPrimitive,
    assets: &mut HashMap<AssetId, AssetSlot>,
    gpu: &Gpu<'_>,
) -> ModelSlot {
    let presence = Arc::downgrade(&primitive.model_presence);

    let slot = match assets.remove(&primitive.asset) {
        Some(slot) => slot,
        None => AssetSlot::from_parse(primitive),
    };

    let AssetSlot {
        presence: asset_presence,
        state,
    } = slot;

    match state {
        AssetState::Failed(error) => {
            assets.insert(
                primitive.asset,
                AssetSlot {
                    presence: asset_presence,
                    state: AssetState::Failed(Arc::clone(&error)),
                },
            );

            failed(primitive, presence, renderer_failure(primitive, error))
        }
        AssetState::Parsed(prepared) => {
            let mapping = match primitive.spec.resolve(prepared.info()) {
                Ok(mapping) => mapping,
                Err(missing) => {
                    assets.insert(
                        primitive.asset,
                        AssetSlot {
                            presence: asset_presence,
                            state: AssetState::Parsed(Box::new(*prepared)),
                        },
                    );

                    return failed(
                        primitive,
                        presence,
                        LoadFailure::Interface {
                            path: primitive.path.as_ref().clone(),
                            missing,
                        },
                    );
                }
            };

            match prepared.upload(gpu.renderer, gpu.device, gpu.queue, primitive.asset_options) {
                Ok(asset) => {
                    let slot = new_ready(primitive, &asset, mapping, presence, gpu);
                    assets.insert(
                        primitive.asset,
                        AssetSlot {
                            presence: asset_presence,
                            state: AssetState::Ready(Box::new(asset)),
                        },
                    );

                    slot
                }
                Err(error) => {
                    let error = Arc::new(error);
                    assets.insert(
                        primitive.asset,
                        AssetSlot {
                            presence: asset_presence,
                            state: AssetState::Failed(Arc::clone(&error)),
                        },
                    );

                    failed(primitive, presence, renderer_failure(primitive, error))
                }
            }
        }
        AssetState::Ready(asset) => {
            let mapping = match primitive.spec.resolve(asset.info()) {
                Ok(mapping) => mapping,
                Err(missing) => {
                    assets.insert(
                        primitive.asset,
                        AssetSlot {
                            presence: asset_presence,
                            state: AssetState::Ready(asset),
                        },
                    );

                    return failed(
                        primitive,
                        presence,
                        LoadFailure::Interface {
                            path: primitive.path.as_ref().clone(),
                            missing,
                        },
                    );
                }
            };

            let slot = new_ready(primitive, &asset, mapping, presence, gpu);
            assets.insert(
                primitive.asset,
                AssetSlot {
                    presence: asset_presence,
                    state: AssetState::Ready(Box::new(*asset)),
                },
            );

            slot
        }
    }
}

fn renderer_failure(primitive: &Live2dPrimitive, error: Arc<Error>) -> LoadFailure {
    LoadFailure::Renderer {
        path: primitive.path.as_ref().clone(),
        error,
    }
}

fn failed(primitive: &Live2dPrimitive, presence: Weak<Presence>, failure: LoadFailure) -> ModelSlot {
    ModelSlot {
        presence,
        asset: primitive.asset,
        listeners: Vec::new(),
        state: ModelState::Failed(failure),
    }
}

/// Creates the instance and its GPU mirror, or fails the slot if the mirror
/// cannot be allocated.
fn new_ready(
    primitive: &Live2dPrimitive,
    asset: &Asset,
    mapping: Mapping,
    presence: Weak<Presence>,
    gpu: &Gpu<'_>,
) -> ModelSlot {
    let model = asset.model();

    let gpu = match ModelGpu::new(gpu.renderer, gpu.device, &model, primitive.model_options.mask_size) {
        Ok(gpu) => gpu,
        Err(error) => {
            return failed(primitive, presence, renderer_failure(primitive, Arc::new(error)));
        }
    };

    ModelSlot {
        presence,
        asset: primitive.asset,
        listeners: Vec::new(),
        state: ModelState::Ready(Box::new(Ready {
            model,
            gpu,
            mapping,
            last: None,
            last_problem: None,
            hover: None,
            down: false,
            drag: None,
            parameters: Vec::new(),
            parts: Vec::new(),
        })),
    }
}

/// Records a listener, replaying the load result if this widget arrived after the
/// model did.
fn register(slot: &mut ModelSlot, events: Option<&Sender<PipelineEvent>>) {
    let Some(events) = events else {
        return;
    };

    if !listen(&mut slot.listeners, events) {
        return;
    }

    let event = match &slot.state {
        ModelState::Ready(ready) => PipelineEvent::Loaded {
            info: Arc::clone(ready.model.info()),
        },
        ModelState::Failed(failure) => PipelineEvent::LoadFailed {
            failure: failure.clone(),
        },
    };

    let _ = events.send(event);
}

/// Records a listener, returning whether it was new.
///
/// A widget that redraws registers again every frame; the channel identity, not
/// the count, decides.
fn listen(
    listeners: &mut Vec<Sender<PipelineEvent>>,
    events: &Sender<PipelineEvent>,
) -> bool {
    if listeners.iter().any(|sender| sender.same_channel(events)) {
        return false;
    }

    listeners.push(events.clone());
    true
}

fn broadcast(listeners: &mut Vec<Sender<PipelineEvent>>, event: PipelineEvent) {
    listeners.retain(|sender| sender.send(event.clone()).is_ok());
}

/// Runs the frame's update order for one model, reporting everything worth
/// reporting.
fn reconcile(
    primitive: &Live2dPrimitive,
    info: &ModelInfo,
    view: &View,
    ready: &mut Ready,
    listeners: &mut Vec<Sender<PipelineEvent>>,
    gpu: &Gpu<'_>,
) -> Cost {
    let inputs = &primitive.inputs;

    ready.model.set_look_at(inputs.look_at);
    ready.model.set_look_at_mode(inputs.look_at_mode);
    ready.model.set_physics_enabled(inputs.physics_enabled);
    ready
        .model
        .set_idle(idle_group(inputs.idle, &primitive.spec, &ready.mapping, info));

    ready.parameters.clear();
    ready
        .parameters
        .extend(inputs.parameters.iter().filter_map(|&(vocabulary, value)| {
            ready
                .mapping
                .parameter(vocabulary)
                .map(|index| (index, value))
        }));
    ready.model.set_parameters(&ready.parameters);

    ready.parts.clear();
    ready
        .parts
        .extend(inputs.parts.iter().filter_map(|&(vocabulary, value)| {
            ready.mapping.part(vocabulary).map(|index| (index, value))
        }));
    ready.model.set_parts(&ready.parts);

    for command in &inputs.commands {
        match *command {
            PrimitiveCommand::PlayMotion { group, clip } => {
                if let Some(group) = ready.mapping.motion(group) {
                    ready.model.play_motion(group, clip);
                }
            }
            PrimitiveCommand::PlayRandomMotion { group } => {
                if let Some(group) = ready.mapping.motion(group) {
                    ready.model.play_random_motion(group);
                }
            }
            PrimitiveCommand::PlayExpression { expression } => {
                if let Some(expression) = ready.mapping.expression(expression) {
                    ready.model.play_expression(expression);
                }
            }
            PrimitiveCommand::StopMotion => ready.model.stop_motion(),
            PrimitiveCommand::ClearExpressions => ready.model.clear_expressions(),
        }
    }

    for problem in &inputs.problems {
        broadcast(listeners, PipelineEvent::Problem(problem.clone()));
    }

    let mut cost = Cost::default();

    if inputs.paused {
        // Time that passes while paused must not be replayed on resume.
        ready.last = None;
    } else {
        let now = Instant::now();
        let delta = ready
            .last
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        ready.last = Some(now);
        cost.delta = delta;

        let started = Instant::now();
        cost.steps = ready.model.advance(delta);
        cost.step_ms = started.elapsed().as_secs_f64() * 1000.0;

        if cost.steps > 0 {
            let started = Instant::now();
            let deformed = ready.model.deform();
            cost.deform_ms = started.elapsed().as_secs_f64() * 1000.0;

            if deformed {
                let started = Instant::now();

                match ready.gpu.upload(gpu.renderer, gpu.device, gpu.queue, &ready.model) {
                    Ok(Some(reason)) => report(&mut ready.last_problem, listeners, reason),
                    Ok(None) => {}
                    Err(error) => report(&mut ready.last_problem, listeners, error),
                }

                cost.upload_ms = started.elapsed().as_secs_f64() * 1000.0;
            } else {
                report(&mut ready.last_problem, listeners, Error::MeshUpdate);
            }
        }

        pointer(primitive, info, view, ready, listeners);
    }

    for started in ready.model.take_started() {
        match motion_key(primitive.asset, info, &ready.mapping, started.group) {
            Some(group) => broadcast(
                listeners,
                PipelineEvent::MotionStarted {
                    group,
                    clip: started.clip,
                    duration: started.duration,
                    looping: started.looping,
                },
            ),
            None => vocabulary_problem("motion", started.group, listeners),
        }
    }

    for finished in ready.model.take_finished() {
        match motion_key(primitive.asset, info, &ready.mapping, finished.group) {
            Some(group) => broadcast(
                listeners,
                PipelineEvent::MotionFinished {
                    group,
                    clip: finished.clip,
                },
            ),
            None => vocabulary_problem("motion", finished.group, listeners),
        }
    }

    for issue in ready.model.take_issues() {
        report(&mut ready.last_problem, listeners, issue);
    }

    cost
}

/// Turns the cursor into hit tests and pointer events.
fn pointer(
    primitive: &Live2dPrimitive,
    info: &ModelInfo,
    view: &View,
    ready: &mut Ready,
    listeners: &mut Vec<Sender<PipelineEvent>>,
) {
    let point = primitive.inputs.cursor.map(|cursor| view.to_model(cursor));
    let hit = point.and_then(|point| ready.model.hit_area(point));

    if hit != ready.hover {
        if let Some(previous) = ready.hover
            && let Some(area) = area_key(primitive.asset, info, &ready.mapping, previous)
        {
            broadcast(listeners, PipelineEvent::Pointer(PointerEventKind::Exited { area }));
        }

        if let Some(next) = hit
            && let Some(area) = area_key(primitive.asset, info, &ready.mapping, next)
        {
            broadcast(
                listeners,
                PipelineEvent::Pointer(PointerEventKind::Entered { area }),
            );
        }

        ready.hover = hit;
    }

    for input in &primitive.inputs.pointer {
        match *input {
            PointerInput::Pressed => {
                let Some(at) = point else {
                    continue;
                };

                ready.down = true;
                ready.drag = Some(at);

                broadcast(
                    listeners,
                    PipelineEvent::Pointer(PointerEventKind::Pressed {
                        area: hit.and_then(|area| {
                            area_key(primitive.asset, info, &ready.mapping, area)
                        }),
                        at,
                    }),
                );
            }
            PointerInput::Released => {
                let Some(at) = point else {
                    continue;
                };

                ready.down = false;
                ready.drag = None;

                broadcast(
                    listeners,
                    PipelineEvent::Pointer(PointerEventKind::Released {
                        area: hit.and_then(|area| {
                            area_key(primitive.asset, info, &ready.mapping, area)
                        }),
                        at,
                    }),
                );
            }
            PointerInput::Scrolled(lines) => broadcast(
                listeners,
                PipelineEvent::Pointer(PointerEventKind::Scrolled { lines }),
            ),
        }
    }

    if let (true, Some(point)) = (ready.down, point) {
        if let Some(previous) = ready.drag
            && previous != point
        {
            broadcast(
                listeners,
                PipelineEvent::Pointer(PointerEventKind::Dragged {
                    delta: [point[0] - previous[0], point[1] - previous[1]],
                }),
            );
        }

        ready.drag = Some(point);
    }
}

/// Reports a failure unless it is the same one as last time.
fn report(
    last_problem: &mut Option<String>,
    listeners: &mut Vec<Sender<PipelineEvent>>,
    error: Error,
) {
    let message = error.to_string();

    if last_problem.as_deref() == Some(message.as_str()) {
        return;
    }

    *last_problem = Some(message);

    broadcast(
        listeners,
        PipelineEvent::Problem(Problem::Renderer {
            error: Arc::new(error),
        }),
    );
}

fn vocabulary_problem(
    vocabulary: &'static str,
    index: usize,
    listeners: &mut Vec<Sender<PipelineEvent>>,
) {
    broadcast(
        listeners,
        PipelineEvent::Problem(Problem::Vocabulary { vocabulary, index }),
    );
}

fn motion_key(
    asset: AssetId,
    info: &ModelInfo,
    mapping: &Mapping,
    group: usize,
) -> Option<KeyId> {
    let index = mapping.motion_vocabulary(group)?;
    let name = Arc::clone(&info.motions.get(group)?.name);

    Some(KeyId { asset, index, name })
}

fn area_key(
    asset: AssetId,
    info: &ModelInfo,
    mapping: &Mapping,
    area: usize,
) -> Option<KeyId> {
    let index = mapping.hit_area_vocabulary(area)?;
    let name = Arc::clone(&info.hit_areas.get(area)?.name);

    Some(KeyId { asset, index, name })
}

/// Resolves what the idle policy means for this interface and this model.
fn idle_group(
    idle: IdleInput,
    spec: &InterfaceSpec,
    mapping: &Mapping,
    info: &ModelInfo,
) -> Option<usize> {
    match idle {
        IdleInput::Off => None,
        IdleInput::Group(vocabulary) => mapping.motion(vocabulary),
        IdleInput::Declared => match spec.motion {
            VocabularySpec::Static(names) => names
                .iter()
                .position(|name| *name == IDLE_GROUP)
                .and_then(|vocabulary| mapping.motion(vocabulary)),
            VocabularySpec::Model => declared_idle(info),
        },
    }
}

fn aspect_of(bounds: &Rectangle) -> f32 {
    if bounds.height > 0.0 {
        bounds.width / bounds.height
    } else {
        1.0
    }
}

/// The GPU handles every load and upload needs.
struct Gpu<'a> {
    renderer: &'a WgpuLive2dRenderer,
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
}

/// What one model's frame cost, for `LIVE2D_PROFILE`.
#[derive(Debug, Clone, Copy, Default)]
struct Cost {
    steps: u32,
    step_ms: f64,
    deform_ms: f64,
    upload_ms: f64,
    delta: Duration,
}

/// Printed once every [`Profile::REPORT_EVERY`] frames when `LIVE2D_PROFILE` is set.
///
/// `steps / frames` well above 1 means the host presents slower than the model
/// steps and is paying for catch-up work. Cost is per frame, which is what a frame
/// budget cares about.
struct Profile {
    frames: u32,
    steps: u32,
    step_ms: f64,
    deform_ms: f64,
    upload_ms: f64,
    delta_ms: f64,
    delta_min: Duration,
    delta_max: Duration,
}

impl Profile {
    const REPORT_EVERY: u32 = 60;

    fn record(&mut self, cost: Cost) -> Option<String> {
        self.frames += 1;
        self.steps += cost.steps;
        self.step_ms += cost.step_ms;
        self.deform_ms += cost.deform_ms;
        self.upload_ms += cost.upload_ms;
        self.delta_ms += cost.delta.as_secs_f64() * 1000.0;
        self.delta_min = self.delta_min.min(cost.delta);
        self.delta_max = self.delta_max.max(cost.delta);

        if self.frames < Self::REPORT_EVERY {
            return None;
        }

        let count = f64::from(self.frames);
        let line = format!(
            "live2d profile: {} frames {:.2} steps, step {:.2} ms, deform {:.2} ms, upload {:.2} ms, delta {:.2}..{:.1} ms (mean {:.2})",
            self.frames,
            f64::from(self.steps) / count,
            self.step_ms / count,
            self.deform_ms / count,
            self.upload_ms / count,
            self.delta_min.as_secs_f64() * 1000.0,
            self.delta_max.as_secs_f64() * 1000.0,
            self.delta_ms / count,
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
            step_ms: 0.0,
            deform_ms: 0.0,
            upload_ms: 0.0,
            delta_ms: 0.0,
            delta_min: Duration::MAX,
            delta_max: Duration::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem() -> PipelineEvent {
        PipelineEvent::Problem(Problem::CommandQueueOverflow { dropped: 1 })
    }

    #[test]
    fn a_listener_is_recorded_once_however_often_it_registers() {
        let (sender, _receiver) = crossbeam_channel::unbounded();
        let mut listeners = Vec::new();

        assert!(listen(&mut listeners, &sender));
        assert!(!listen(&mut listeners, &sender.clone()));
        assert_eq!(listeners.len(), 1);
    }

    #[test]
    fn every_live_listener_gets_the_event() {
        let (first, first_receiver) = crossbeam_channel::unbounded();
        let (second, second_receiver) = crossbeam_channel::unbounded();
        let mut listeners = vec![first, second];

        broadcast(&mut listeners, problem());

        assert!(first_receiver.try_recv().is_ok());
        assert!(second_receiver.try_recv().is_ok());
    }

    #[test]
    fn a_listener_whose_widget_left_the_tree_is_dropped() {
        let (dead, dead_receiver) = crossbeam_channel::unbounded();
        let (live, live_receiver) = crossbeam_channel::unbounded::<PipelineEvent>();
        drop(dead_receiver);

        let mut listeners = vec![dead, live];
        broadcast(&mut listeners, problem());

        assert_eq!(listeners.len(), 1);
        assert!(live_receiver.try_recv().is_ok());
    }
}

