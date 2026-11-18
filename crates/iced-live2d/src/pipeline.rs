//! The render layer's side of the component: one `Pipeline`, one primitive.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use iced::Rectangle;
use iced::wgpu;
use iced::widget::shader;
use live2d_render::{ClipRect, Live2dScene, LookAtMode};

use crate::event::{Live2dEvent, LoadFailure, ModelSummary, MotionId, Problem};
use crate::model::{Command, Presence, RenderSettings};

/// One frame of one model's inputs.
///
/// Rebuilt by the widget on every frame and dropped after it is rendered, so it
/// carries plain data only — no GPU state, no `Rc`, nothing that would make the
/// render path need a lock.
#[derive(Clone, Debug)]
pub struct Live2dPrimitive {
    pub(crate) id: u64,
    pub(crate) path: Arc<PathBuf>,
    pub(crate) settings: RenderSettings,
    /// Resolved LookAt target for this frame: the widget's source if it had one,
    /// otherwise the target the host set on the model.
    pub(crate) look_at: Option<[f32; 2]>,
    pub(crate) look_at_mode: LookAtMode,
    pub(crate) physics_enabled: bool,
    pub(crate) paused: bool,
    pub(crate) fill: f32,
    pub(crate) commands: Vec<Command>,
    pub(crate) presence: Arc<Presence>,
    pub(crate) events: Sender<Live2dEvent>,
}

impl Live2dPrimitive {
    fn send(&self, event: Live2dEvent) {
        // The receiver only exists while the widget is in the tree; a model
        // drawn without `on_event` has nobody to tell.
        let _ = self.events.send(event);
    }
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
        let format = pipeline.format;

        if let Entry::Vacant(entry) = pipeline.slots.entry(self.id) {
            let slot = self.load(device, queue, format);
            entry.insert(slot);
        }

        let Some(Slot::Ready(ready)) = pipeline.slots.get_mut(&self.id) else {
            return;
        };

        ready.reconcile(self, queue, bounds);

        if self.paused {
            // Time that passes while paused must not be replayed on resume.
            ready.last = None;
            return;
        }

        let now = Instant::now();
        let delta = ready
            .last
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        ready.last = Some(now);

        match ready.scene.advance(device, queue, delta) {
            Ok(()) => ready.last_problem = None,
            Err(error) => ready.report(&self.events, error),
        }

        for issue in ready.scene.take_issues() {
            self.send(Live2dEvent::ModelReportedProblem {
                problem: Problem::Renderer { error: issue },
            });
        }

        for index in ready.scene.take_finished_motions() {
            self.send(Live2dEvent::MotionFinishedPlaying {
                motion: MotionId { index },
            });
        }
    }

    fn render(
        &self,
        pipeline: &Live2dPipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let Some(Slot::Ready(ready)) = pipeline.slots.get(&self.id) else {
            return;
        };

        let clip = ClipRect {
            x: clip_bounds.x,
            y: clip_bounds.y,
            width: clip_bounds.width,
            height: clip_bounds.height,
        };

        match ready.scene.encode(encoder, target, clip) {
            Ok(()) => ready.render_problem.store(false, Ordering::Relaxed),
            Err(error) => {
                if !ready.render_problem.swap(true, Ordering::Relaxed) {
                    self.send(Live2dEvent::ModelReportedProblem {
                        problem: Problem::Renderer { error },
                    });
                }
            }
        }
    }
}

impl Live2dPrimitive {
    fn load(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Slot {
        let presence = Arc::downgrade(&self.presence);

        match Live2dScene::load_with_options(
            device,
            queue,
            self.path.as_path(),
            format,
            self.settings.to_scene_options(),
        ) {
            Ok(scene) => {
                self.send(Live2dEvent::ModelLoaded {
                    summary: summarize(&scene),
                });

                Slot::Ready(Box::new(Ready {
                    scene,
                    presence,
                    last: None,
                    last_problem: None,
                    render_problem: AtomicBool::new(false),
                }))
            }
            Err(error) => {
                self.send(Live2dEvent::ModelFailedToLoad {
                    failure: LoadFailure {
                        path: self.path.as_ref().clone(),
                        error,
                    },
                });

                Slot::Failed { presence }
            }
        }
    }
}

/// What one model occupies: its scene, and the bookkeeping the pipeline needs.
struct Ready {
    scene: Live2dScene,
    presence: Weak<Presence>,
    last: Option<Instant>,
    /// Last `advance` failure, so a failure that repeats every frame is reported
    /// once instead of sixty times a second.
    last_problem: Option<String>,
    /// Whether `render` already reported its current failure.
    render_problem: AtomicBool,
}

impl Ready {
    fn reconcile(&mut self, primitive: &Live2dPrimitive, queue: &wgpu::Queue, bounds: &Rectangle) {
        let scene = &mut self.scene;

        scene.set_look_at_mode(primitive.look_at_mode);
        scene.set_physics_enabled(primitive.physics_enabled);
        scene.set_fill(queue, primitive.fill * 2.0);

        if bounds.width > 0.0 && bounds.height > 0.0 {
            scene.resize(queue, bounds.width / bounds.height);
        }

        if let Some(target) = primitive.look_at {
            scene.look_at(target);
        }

        for command in &primitive.commands {
            match *command {
                Command::PlayMotion(index) => scene.play_motion(index),
                Command::PlayExpression(index) => scene.play_expression(index),
                Command::StopMotion => scene.stop_motion(),
            }
        }
    }

    fn report(&mut self, events: &Sender<Live2dEvent>, error: live2d_render::Error) {
        let message = error.to_string();

        if self.last_problem.as_deref() == Some(message.as_str()) {
            return;
        }

        self.last_problem = Some(message);

        let _ = events.send(Live2dEvent::ModelReportedProblem {
            problem: Problem::Renderer { error },
        });
    }
}

enum Slot {
    Ready(Box<Ready>),
    /// Loading failed, and is not retried while the model lives.
    Failed {
        presence: Weak<Presence>,
    },
}

impl Slot {
    fn is_alive(&self) -> bool {
        let presence = match self {
            Slot::Ready(ready) => &ready.presence,
            Slot::Failed { presence } => presence,
        };

        presence.strong_count() > 0
    }
}

/// Per-model GPU state, shared by every widget that draws a model.
///
/// iced creates exactly one of these per primitive type, so per-model state has
/// to be keyed — here by model identity.
pub struct Live2dPipeline {
    format: wgpu::TextureFormat,
    slots: HashMap<u64, Slot>,
}

impl shader::Pipeline for Live2dPipeline {
    fn new(_device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self {
            format,
            slots: HashMap::new(),
        }
    }

    fn trim(&mut self) {
        self.slots.retain(|_, slot| slot.is_alive());
    }
}

fn summarize(scene: &Live2dScene) -> ModelSummary {
    ModelSummary {
        motions: scene.motion_count(),
        motion_groups: scene.motion_groups().count(),
        expressions: scene.expression_count(),
    }
}
