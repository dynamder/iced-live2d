//! iced glue for the Live2D render layer.
//!
//! All persistent state lives in [`Live2dPipeline`], which iced creates once and
//! hands to us as `&mut` in [`shader::Primitive::prepare`]. The per-frame
//! [`Live2dPrimitive`] only carries inputs (time, cursor, model path) and has no
//! GPU state, which keeps it cheap to rebuild every frame.

use std::path::PathBuf;
use std::time::Instant;

use iced::mouse;
use iced::wgpu;
use iced::widget::shader;
use iced::{Point, Rectangle};
use live2d_render::{ClipRect, Live2dScene, SceneOptions};

use crate::Message;

/// One-shot actions the UI asks the render layer to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live2dCommand {
    NextMotion,
    NextExpression,
}

pub struct Live2dView {
    model_path: PathBuf,
    options: SceneOptions,
    now: Instant,
    command: Option<Live2dCommand>,
    command_seq: u64,
}

impl Live2dView {
    pub fn new(model_path: PathBuf, options: SceneOptions) -> Self {
        Self {
            model_path,
            options,
            now: Instant::now(),
            command: None,
            command_seq: 0,
        }
    }

    pub fn set_now(&mut self, now: Instant) {
        self.now = now;
    }

    /// Queues a command for the render layer. The sequence number is what makes
    /// the same command fire once instead of on every frame.
    pub fn send(&mut self, command: Live2dCommand) {
        self.command = Some(command);
        self.command_seq = self.command_seq.wrapping_add(1);
    }
}

#[derive(Debug)]
pub struct Live2dPrimitive {
    model_path: PathBuf,
    options: SceneOptions,
    now: Instant,
    gaze: Option<[f32; 2]>,
    command: Option<(Live2dCommand, u64)>,
}

impl shader::Program<Message> for Live2dView {
    type State = ();
    type Primitive = Live2dPrimitive;

    fn draw(&self, _state: &(), cursor: mouse::Cursor, bounds: Rectangle) -> Live2dPrimitive {
        Live2dPrimitive {
            model_path: self.model_path.clone(),
            options: self.options,
            now: self.now,
            gaze: gaze_from_cursor(cursor, bounds),
            command: self.command.map(|command| (command, self.command_seq)),
        }
    }
}

/// Maps the cursor inside the widget to `-1.0..=1.0` on both axes.
fn gaze_from_cursor(cursor: mouse::Cursor, bounds: Rectangle) -> Option<[f32; 2]> {
    let position: Point = cursor.position_in(bounds)?;

    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return None;
    }

    Some([
        (position.x / bounds.width) * 2.0 - 1.0,
        1.0 - (position.y / bounds.height) * 2.0,
    ])
}

/// Persistent GPU state, created by iced once and reused by every primitive.
///
/// The model is loaded lazily in [`shader::Primitive::prepare`], the only place
/// that has both the device and the model path.
pub struct Live2dPipeline {
    format: wgpu::TextureFormat,
    scene: Option<Live2dScene>,
    failure: Option<String>,
    last_frame: Option<Instant>,
    last_command_seq: u64,
}

impl shader::Pipeline for Live2dPipeline {
    fn new(_device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        Self {
            format,
            scene: None,
            failure: None,
            last_frame: None,
            last_command_seq: 0,
        }
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
        if pipeline.scene.is_none() && pipeline.failure.is_none() {
            let loaded = Live2dScene::load_with_options(
                device,
                queue,
                &self.model_path,
                pipeline.format,
                self.options,
            );

            match loaded {
                Ok(mut scene) => {
                    println!(
                        "live2d: loaded {} ({} motions in {} groups, {} expressions, target {:?})",
                        self.model_path.display(),
                        scene.motion_count(),
                        scene.motion_groups().count(),
                        scene.expression_count(),
                        pipeline.format,
                    );
                    println!(
                        "live2d: tracking ({}) {}",
                        self.options.tracking,
                        scene
                            .rig()
                            .bindings()
                            .iter()
                            .map(|binding| format!("{}={}", binding.slot.name(), binding.parameter))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    report_issues(&mut scene);
                    pipeline.scene = Some(scene);
                }
                Err(error) => {
                    eprintln!("live2d: failed to load model: {error}");
                    pipeline.failure = Some(error.to_string());
                }
            }
        }

        let Some(scene) = pipeline.scene.as_mut() else {
            return;
        };

        let delta = pipeline
            .last_frame
            .map(|last| self.now.saturating_duration_since(last))
            .unwrap_or_default();
        pipeline.last_frame = Some(self.now);

        if bounds.height > 0.0 {
            scene.resize(queue, bounds.width / bounds.height);
        }

        if let Some(gaze) = self.gaze {
            scene.set_gaze(gaze);
        }

        if let Some((command, seq)) = self.command
            && seq != pipeline.last_command_seq
        {
            pipeline.last_command_seq = seq;

            match command {
                Live2dCommand::NextMotion => scene.play_next_motion(),
                Live2dCommand::NextExpression => {
                    scene.play_next_expression();
                }
            }
        }

        let advance_result = scene.advance(device, queue, delta);

        if let Err(error) = advance_result {
            eprintln!("live2d: advance failed: {error}");
        }

        report_issues(scene);
    }

    fn render(
        &self,
        pipeline: &Live2dPipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let Some(scene) = pipeline.scene.as_ref() else {
            return;
        };

        let clip = ClipRect {
            x: clip_bounds.x,
            y: clip_bounds.y,
            width: clip_bounds.width,
            height: clip_bounds.height,
        };

        if let Err(error) = scene.encode(encoder, target, clip) {
            eprintln!("live2d: encode failed: {error}");
        }
    }
}

/// Prints the problems the scene decided not to abort on (broken sidecars,
/// unreadable animation directories).
fn report_issues(scene: &mut Live2dScene) {
    for issue in scene.take_issues() {
        eprintln!("live2d: {issue}");
    }
}
