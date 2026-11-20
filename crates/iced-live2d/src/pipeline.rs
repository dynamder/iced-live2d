//! The render layer's side of the component: one pipeline per primitive type,
//! holding the per-asset, per-model and per-view state.
//!
//! A slot is reclaimed when the host drops what it stands for: each holds a `Weak`
//! to a `Presence` the host object owns.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use iced::wgpu;
use iced::widget::shader;
use iced::{Point, Rectangle};
use live2d_render::{
    Asset, AssetId, AssetOptions, ClipRect, Error, Fade, HitAreaIndex, LookAtMode, Model, ModelGpu,
    ModelInfo, ModelOptions, MotionClipIndex, MotionEnd, MotionGroupIndex, ParameterIndex,
    ParameterWrite, PartIndex, Prepared, View, WgpuLive2dRenderer,
};

use crate::event::{LoadFailure, Problem};
use crate::interaction::{Pointer, Sample};
use crate::interface::{InterfaceSpec, KeyId, Mapping};
use crate::look_at;
use crate::model::{Live2dId, PointerInput, Presence};

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
    Pressed {
        pointer: Pointer,
        area: Option<KeyId>,
        at: Sample,
        when: Instant,
    },
    Released {
        pointer: Pointer,
        area: Option<KeyId>,
        at: Sample,
        when: Instant,
    },
    Cancelled {
        pointer: Pointer,
        when: Instant,
    },
    Moved {
        at: Sample,
        when: Instant,
    },
    Entered {
        area: KeyId,
        at: Sample,
        when: Instant,
    },
    Exited {
        area: KeyId,
        at: Sample,
        when: Instant,
    },
    Dragged {
        pointer: Pointer,
        area: Option<KeyId>,
        origin: Sample,
        at: Sample,
        when: Instant,
    },
    Scrolled {
        lines: [f32; 2],
        at: Sample,
        when: Instant,
    },
}

/// Something the pipeline tells the widgets that draw a model. Non-generic: the
/// widget turns these [`KeyId`]s back into typed keys.
#[derive(Debug, Clone)]
pub(crate) enum PipelineEvent {
    /// A model event, broadcast to every widget drawing the model.
    Model(ModelEventKind),
    /// A pointer event, delivered to the widget whose view produced it.
    Pointer(PointerEventKind),
}

/// A model event before the widget types its keys.
#[derive(Debug, Clone)]
pub(crate) enum ModelEventKind {
    Loaded {
        info: Arc<ModelInfo>,
    },
    LoadFailed {
        failure: LoadFailure,
    },
    Problem(Problem),
    MotionStarted {
        group: KeyId,
        clip: MotionClipIndex,
        duration: f32,
        looping: bool,
    },
    MotionEnded {
        group: KeyId,
        clip: MotionClipIndex,
        ended: MotionEnd,
    },
}

/// Something the host asked the model to do, reduced to vocabulary indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrimitiveCommand {
    PlayMotion { group: usize, clip: MotionClipIndex },
    PlayRandomMotion { group: usize },
    PlayExpression { expression: usize },
    StopMotion,
    ClearExpressions,
}

/// One frame of one model's inputs. Plain data only, rebuilt every frame, so the
/// render path needs no lock.
#[derive(Debug)]
pub struct Live2dPrimitive {
    pub(crate) asset: AssetId,
    pub(crate) asset_options: AssetOptions,
    pub(crate) path: Arc<PathBuf>,
    pub(crate) model: Live2dId,
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
    pub(crate) parameters: Vec<(usize, ParameterWrite)>,
    pub(crate) parts: Vec<(usize, f32)>,
    pub(crate) commands: Vec<PrimitiveCommand>,
    pub(crate) problems: Vec<Problem>,
    pub(crate) look_at: Option<[f32; 2]>,
    pub(crate) look_at_mode: LookAtMode,
    /// A host-chosen fade, overriding whatever the model files declare.
    pub(crate) fade: Option<Fade>,
    pub(crate) fade_limit: Option<usize>,
    pub(crate) physics_enabled: bool,
    pub(crate) frozen: bool,
    /// Where the cursor is, in window pixels, when it is over the widget.
    pub(crate) cursor: Option<Point>,
    /// Where the pointer is, in window pixels, inside the widget or not: a drag
    /// that leaves the widget keeps reporting.
    pub(crate) position: Option<Point>,
    pub(crate) pointer: Vec<PointerInput>,
    /// Whether this widget asked for the per-frame cost line.
    pub(crate) profile: bool,
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
    models: HashMap<Live2dId, ModelSlot>,
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
            profile: None,
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
        self.models
            .retain(|_, slot| slot.presence.strong_count() > 0);
        self.views
            .retain(|_, slot| slot.presence.strong_count() > 0);

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
                listener: None,
                interaction: Interaction::default(),
            });

            slot.presence = Arc::downgrade(&primitive.view_presence);
            slot.listener = primitive.events.clone();
            slot.view.set_aspect(queue, aspect);
            slot.view.set_fill(queue, primitive.fill);
        }

        if let Some(slot) = models.get_mut(&primitive.model) {
            register(slot, primitive.events.as_ref());
        }

        let Some(asset) = assets.get(&primitive.asset) else {
            return;
        };

        let AssetState::Ready(asset) = &asset.state else {
            return;
        };

        let Some(view) = views.get_mut(&primitive.view) else {
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

        let cost = reconcile(
            primitive,
            asset.info(),
            bounds,
            view,
            ready,
            listeners,
            &gpu,
        );

        if primitive.inputs.profile
            && let Some(line) = profile.get_or_insert_with(Profile::default).record(cost)
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
        let (
            Some(AssetSlot {
                state: AssetState::Ready(asset),
                ..
            }),
            Some(ModelSlot {
                state: ModelState::Ready(ready),
                ..
            }),
            Some(view),
        ) = (
            self.assets.get(&primitive.asset),
            self.models.get(&primitive.model),
            self.views.get(&primitive.view),
        )
        else {
            return;
        };

        let clip = ClipRect {
            x: clip_bounds.x,
            y: clip_bounds.y,
            width: clip_bounds.width,
            height: clip_bounds.height,
        };

        if let Err(error) = ready.gpu.encode(
            &self.renderer,
            encoder,
            target,
            clip,
            asset.textures(),
            &view.view,
        ) {
            self.push_render_failure(primitive.model, error);
        }
    }

    fn push_render_failure(&self, model: Live2dId, error: Error) {
        let message = error.to_string();
        let mut pending = lock(&self.pending);

        if pending.render_failures.get(&model) == Some(&message) {
            return;
        }

        pending.render_failures.insert(model, message);
        pending.events.push((
            model,
            PipelineEvent::Model(ModelEventKind::Problem(Problem::Renderer {
                error: Arc::new(error),
            })),
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
    events: Vec<(Live2dId, PipelineEvent)>,
    /// Last render failure per model, so a failure that repeats every frame is
    /// reported once.
    render_failures: HashMap<Live2dId, String>,
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

        let state = match Asset::parse(
            primitive.asset,
            primitive.path.as_path(),
            primitive.asset_options,
        ) {
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
    /// The widget this framing belongs to, if it wants events at all. Pointer
    /// events go here only: they happen in this view's space.
    listener: Option<Sender<PipelineEvent>>,
    interaction: Interaction,
}

/// Everything a pointer does here, per view: two widgets on one model never see
/// each other's presses or hover.
#[derive(Default)]
struct Interaction {
    /// The hit area under the cursor, as a model index.
    hover: Option<HitAreaIndex>,
    /// Every pointer currently held down, at most a handful.
    down: Vec<Down>,
    /// The last position reported as a `Moved`, in window pixels: inside only, so
    /// a cursor that never enters the widget reports nothing.
    moved: Option<Point>,
}

struct Down {
    pointer: Pointer,
    /// The hit area under the press, if any: a drag reports where it started.
    area: Option<HitAreaIndex>,
    origin: Point,
    /// Where the pointer is now, from the frame cursor or from a touch move.
    current: Point,
    /// What was last reported as a drag.
    reported: Point,
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
    parameters: Vec<(ParameterIndex, ParameterWrite)>,
    parts: Vec<(PartIndex, f32)>,
    /// Asset problems found while loading, reported on registration.
    issues: Vec<Arc<Error>>,
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
                Ok(mut asset) => {
                    let issues = asset.take_issues().into_iter().map(Arc::new).collect();
                    let slot = new_ready(primitive, &asset, mapping, presence, gpu, issues);
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

            let slot = new_ready(primitive, &asset, mapping, presence, gpu, Vec::new());
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

fn failed(
    primitive: &Live2dPrimitive,
    presence: Weak<Presence>,
    failure: LoadFailure,
) -> ModelSlot {
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
    issues: Vec<Arc<Error>>,
) -> ModelSlot {
    let model = asset.model();

    let gpu = match ModelGpu::new(
        gpu.renderer,
        gpu.device,
        &model,
        primitive.model_options.mask_size,
    ) {
        Ok(gpu) => gpu,
        Err(error) => {
            return failed(
                primitive,
                presence,
                renderer_failure(primitive, Arc::new(error)),
            );
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
            parameters: Vec::new(),
            parts: Vec::new(),
            issues,
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
        ModelState::Ready(ready) => PipelineEvent::Model(ModelEventKind::Loaded {
            info: Arc::clone(ready.model.info()),
        }),
        ModelState::Failed(failure) => PipelineEvent::Model(ModelEventKind::LoadFailed {
            failure: failure.clone(),
        }),
    };

    let _ = events.send(event);

    if let ModelState::Ready(ready) = &slot.state {
        for issue in &ready.issues {
            let _ = events.send(PipelineEvent::Model(ModelEventKind::Problem(
                Problem::Asset {
                    error: Arc::clone(issue),
                },
            )));
        }
    }
}

/// Records a listener, returning whether it was new.
///
/// A widget that redraws registers again every frame; the channel identity, not
/// the count, decides.
fn listen(listeners: &mut Vec<Sender<PipelineEvent>>, events: &Sender<PipelineEvent>) -> bool {
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
    bounds: &Rectangle,
    view: &mut ViewSlot,
    ready: &mut Ready,
    listeners: &mut Vec<Sender<PipelineEvent>>,
    gpu: &Gpu<'_>,
) -> Cost {
    let inputs = &primitive.inputs;

    ready.model.set_look_at(inputs.look_at);
    ready.model.set_look_at_mode(inputs.look_at_mode);
    ready.model.set_physics_enabled(inputs.physics_enabled);
    ready.model.set_fade(inputs.fade);
    ready.model.set_fade_limit(inputs.fade_limit);
    ready
        .model
        .set_parameter_policy(primitive.model_options.parameter_policy);
    ready
        .model
        .set_look_at_blend(primitive.model_options.look_at_blend);

    ready.parameters.clear();

    for &(vocabulary, write) in &inputs.parameters {
        match ready.mapping.parameter(vocabulary) {
            Some(index) => ready.parameters.push((index, write)),
            None => vocabulary_problem("parameter", vocabulary, listeners),
        }
    }

    ready.model.set_parameters(&ready.parameters);

    ready.parts.clear();

    for &(vocabulary, value) in &inputs.parts {
        match ready.mapping.part(vocabulary) {
            Some(index) => ready.parts.push((index, value)),
            None => vocabulary_problem("part", vocabulary, listeners),
        }
    }

    ready.model.set_parts(&ready.parts);

    for command in &inputs.commands {
        match *command {
            PrimitiveCommand::PlayMotion { group, clip } => match ready.mapping.motion(group) {
                Some(index) => ready.model.play_motion(index, clip),
                None => vocabulary_problem("motion", group, listeners),
            },
            PrimitiveCommand::PlayRandomMotion { group } => match ready.mapping.motion(group) {
                Some(index) => ready.model.play_random_motion(index),
                None => vocabulary_problem("motion", group, listeners),
            },
            PrimitiveCommand::PlayExpression { expression } => {
                match ready.mapping.expression(expression) {
                    Some(index) => ready.model.play_expression(index),
                    None => vocabulary_problem("expression", expression, listeners),
                }
            }
            PrimitiveCommand::StopMotion => ready.model.stop_motion(),
            PrimitiveCommand::ClearExpressions => ready.model.clear_expressions(),
        }
    }

    for problem in &inputs.problems {
        broadcast(
            listeners,
            PipelineEvent::Model(ModelEventKind::Problem(problem.clone())),
        );
    }

    let mut cost = Cost::default();

    if inputs.frozen {
        // Time that passes while frozen must not be replayed on resume.
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

                match ready
                    .gpu
                    .upload(gpu.renderer, gpu.device, gpu.queue, &ready.model)
                {
                    Ok(Some(reason)) => report(&mut ready.last_problem, listeners, reason),
                    Ok(None) => {}
                    Err(error) => report(&mut ready.last_problem, listeners, error),
                }

                cost.upload_ms = started.elapsed().as_secs_f64() * 1000.0;
            } else {
                report(&mut ready.last_problem, listeners, Error::MeshUpdate);
            }
        }
    }

    pointer(primitive, info, bounds, view, ready);

    for started in ready.model.take_started() {
        match motion_key(primitive.asset, info, &ready.mapping, started.group) {
            Some(group) => broadcast(
                listeners,
                PipelineEvent::Model(ModelEventKind::MotionStarted {
                    group,
                    clip: started.clip,
                    duration: started.duration,
                    looping: started.looping,
                }),
            ),
            None => vocabulary_problem("motion", started.group.get(), listeners),
        }
    }

    for ended in ready.model.take_ended() {
        match motion_key(primitive.asset, info, &ready.mapping, ended.group) {
            Some(group) => broadcast(
                listeners,
                PipelineEvent::Model(ModelEventKind::MotionEnded {
                    group,
                    clip: ended.clip,
                    ended: ended.reason,
                }),
            ),
            None => vocabulary_problem("motion", ended.group.get(), listeners),
        }
    }

    for issue in ready.model.take_issues() {
        report(&mut ready.last_problem, listeners, issue);
    }

    cost
}

/// Turns this frame's pointer state into hit tests and pointer events.
///
/// Runs for this view only, so two widgets on one model never see each other's
/// presses or hover. It also runs while the model is frozen: nothing steps, but
/// the pose on screen is still hit-testable.
///
/// Positions are window pixels throughout, so the events do not move when the
/// widget's own rectangle does.
fn pointer(
    primitive: &Live2dPrimitive,
    info: &ModelInfo,
    bounds: &Rectangle,
    view: &mut ViewSlot,
    ready: &mut Ready,
) {
    let ViewSlot {
        view,
        listener,
        interaction,
        ..
    } = view;

    let listener = listener.as_ref();

    let sample = |point: Point| {
        let clip = look_at::clip_at(point, *bounds);

        clip.map(|clip| Sample {
            pixels: Point::new(point.x - bounds.x, point.y - bounds.y),
            clip,
            model: view.to_model(clip),
        })
    };

    if let Some(position) = primitive.inputs.cursor
        && interaction.moved != Some(position)
    {
        interaction.moved = Some(position);

        if let Some(at) = sample(position) {
            send(
                listener,
                PipelineEvent::Pointer(PointerEventKind::Moved {
                    at,
                    when: Instant::now(),
                }),
            );
        }
    }

    let hover = primitive.inputs.cursor.and_then(|position| {
        let clip = look_at::clip_at(position, *bounds)?;

        ready.model.hit_area(view.to_model(clip))
    });

    if hover != interaction.hover {
        let at = interaction
            .moved
            .or(primitive.inputs.cursor)
            .and_then(sample);

        if let Some(previous) = interaction.hover
            && let (Some(area), Some(at)) = (
                area_key(primitive.asset, info, &ready.mapping, previous),
                at,
            )
        {
            send(
                listener,
                PipelineEvent::Pointer(PointerEventKind::Exited {
                    area,
                    at,
                    when: Instant::now(),
                }),
            );
        }

        if let Some(next) = hover
            && let (Some(area), Some(at)) =
                (area_key(primitive.asset, info, &ready.mapping, next), at)
        {
            send(
                listener,
                PipelineEvent::Pointer(PointerEventKind::Entered {
                    area,
                    at,
                    when: Instant::now(),
                }),
            );
        }

        interaction.hover = hover;
    }

    for input in &primitive.inputs.pointer {
        match *input {
            PointerInput::Pressed { pointer, at, when } => {
                let Some(pressed) = sample(at) else {
                    continue;
                };

                let area = ready.model.hit_area(pressed.model);

                interaction.down.retain(|down| down.pointer != pointer);
                interaction.down.push(Down {
                    pointer,
                    area,
                    origin: at,
                    current: at,
                    reported: at,
                });

                send(
                    listener,
                    PipelineEvent::Pointer(PointerEventKind::Pressed {
                        pointer,
                        area: area
                            .and_then(|area| area_key(primitive.asset, info, &ready.mapping, area)),
                        at: pressed,
                        when,
                    }),
                );
            }
            PointerInput::Released { pointer, at, when } => {
                let Some(released) = sample(at) else {
                    continue;
                };

                let Some(index) = interaction
                    .down
                    .iter()
                    .position(|down| down.pointer == pointer)
                else {
                    continue;
                };

                interaction.down.remove(index);

                let area = ready.model.hit_area(released.model);

                send(
                    listener,
                    PipelineEvent::Pointer(PointerEventKind::Released {
                        pointer,
                        area: area
                            .and_then(|area| area_key(primitive.asset, info, &ready.mapping, area)),
                        at: released,
                        when,
                    }),
                );
            }
            PointerInput::Scrolled { lines, at, when } => {
                if let Some(at) = sample(at) {
                    send(
                        listener,
                        PipelineEvent::Pointer(PointerEventKind::Scrolled { lines, at, when }),
                    );
                }
            }
            PointerInput::CancelAll => {
                for down in interaction.down.drain(..) {
                    send(
                        listener,
                        PipelineEvent::Pointer(PointerEventKind::Cancelled {
                            pointer: down.pointer,
                            when: Instant::now(),
                        }),
                    );
                }
            }
        }
    }

    if let Some(position) = primitive.inputs.position {
        for down in &mut interaction.down {
            if matches!(down.pointer, Pointer::Mouse(_)) {
                down.current = position;
            }
        }
    }

    for down in &mut interaction.down {
        if down.current == down.reported {
            continue;
        }

        down.reported = down.current;

        let (Some(origin), Some(at)) = (sample(down.origin), sample(down.current)) else {
            continue;
        };

        send(
            listener,
            PipelineEvent::Pointer(PointerEventKind::Dragged {
                pointer: down.pointer,
                area: down
                    .area
                    .and_then(|area| area_key(primitive.asset, info, &ready.mapping, area)),
                origin,
                at,
                when: Instant::now(),
            }),
        );
    }
}

/// Hands one event to this view's widget, if it listens.
fn send(listener: Option<&Sender<PipelineEvent>>, event: PipelineEvent) {
    if let Some(listener) = listener {
        let _ = listener.send(event);
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
        PipelineEvent::Model(ModelEventKind::Problem(Problem::Renderer {
            error: Arc::new(error),
        })),
    );
}

fn vocabulary_problem(
    vocabulary: &'static str,
    index: usize,
    listeners: &mut Vec<Sender<PipelineEvent>>,
) {
    broadcast(
        listeners,
        PipelineEvent::Model(ModelEventKind::Problem(Problem::Vocabulary {
            vocabulary,
            index,
        })),
    );
}

fn motion_key(
    asset: AssetId,
    info: &ModelInfo,
    mapping: &Mapping,
    group: MotionGroupIndex,
) -> Option<KeyId> {
    let index = mapping.motion_vocabulary(group)?;
    let name = Arc::clone(&info.motions.get(group.get())?.name);

    Some(KeyId { asset, index, name })
}

fn area_key(
    asset: AssetId,
    info: &ModelInfo,
    mapping: &Mapping,
    area: HitAreaIndex,
) -> Option<KeyId> {
    let index = mapping.hit_area_vocabulary(area)?;
    let name = Arc::clone(&info.hit_areas.get(area.get())?.name);

    Some(KeyId { asset, index, name })
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

/// What one model's frame cost, when a widget asks to be profiled.
#[derive(Debug, Clone, Copy, Default)]
struct Cost {
    steps: u32,
    step_ms: f64,
    deform_ms: f64,
    upload_ms: f64,
    delta: Duration,
}

/// The per-frame cost line, printed every [`Profile::REPORT_EVERY`] frames.
///
/// `steps / frames` well above 1 means the model steps faster than the host
/// presents.
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
        PipelineEvent::Model(ModelEventKind::Problem(Problem::CommandQueueOverflow {
            dropped: 1,
        }))
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
