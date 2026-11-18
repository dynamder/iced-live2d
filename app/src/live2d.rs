//! iced glue for the Live2D render layer.
//!
//! All persistent state lives in [`Live2dPipeline`], which iced creates once and
//! hands to us as `&mut` in [`shader::Primitive::prepare`]. The per-frame
//! [`Live2dPrimitive`] only carries inputs (time, cursor, model path) and has no
//! GPU state, which keeps it cheap to rebuild every frame.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use iced::mouse;
use iced::widget::shader;
use iced::wgpu;
use iced::{Point, Rectangle};
use live2d_render::{ClipRect, Live2dScene};

use crate::Message;

/// One-shot actions the UI asks the render layer to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live2dCommand {
    NextMotion,
    NextExpression,
}

pub struct Live2dView {
    model_path: PathBuf,
    now: Instant,
    command: Option<Live2dCommand>,
    command_seq: u64,
}

impl Live2dView {
    pub fn new(model_path: PathBuf) -> Self {
        Self {
            model_path,
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

pub struct Live2dPipeline {
    format: wgpu::TextureFormat,
    scene: Option<Live2dScene>,
    failure: Option<String>,
    last_frame: Option<Instant>,
    last_command_seq: u64,
    // TEMP diagnostics
    prepares: u64,
    renders: AtomicU64,
    distinct_now: u64,
    last_now: Option<Instant>,
    report_start: Instant,
    profile: bool,
    advance_nanos: u128,
}

impl shader::Pipeline for Live2dPipeline {
    fn new(
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        // The model is loaded lazily in `prepare`, the only place that has both
        // the device and the model path.
        println!("live2d: iced render target format is {format:?}");

        Self {
            format,
            scene: None,
            failure: None,
            last_frame: None,
            last_command_seq: 0,
            prepares: 0,
            renders: AtomicU64::new(0),
            distinct_now: 0,
            last_now: None,
            report_start: Instant::now(),
            profile: std::env::var_os("LIVE2D_PROFILE").is_some(),
            advance_nanos: 0,
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
        pipeline.prepares += 1;

        if pipeline.last_now != Some(self.now) {
            pipeline.distinct_now += 1;
            pipeline.last_now = Some(self.now);
        }

        let report_elapsed = Instant::now().saturating_duration_since(pipeline.report_start);
        if pipeline.profile && report_elapsed >= std::time::Duration::from_secs(1) {
            println!(
                "live2d diag: prepares={} renders={} distinct_now={} in {:.2}s",
                pipeline.prepares,
                pipeline.renders.load(Ordering::Relaxed),
                pipeline.distinct_now,
                report_elapsed.as_secs_f32(),
            );
            println!(
                "live2d diag: advance averaged {:.2} ms/frame",
                pipeline.advance_nanos as f64 / 1e6 / (pipeline.prepares.max(1) as f64),
            );
            pipeline.advance_nanos = 0;
            pipeline.prepares = 0;
            pipeline.renders.store(0, Ordering::Relaxed);
            pipeline.distinct_now = 0;
            pipeline.report_start = Instant::now();
        }

        if pipeline.scene.is_none() && pipeline.failure.is_none() {
            match Live2dScene::load(device, queue, &self.model_path, pipeline.format) {
                Ok(scene) => {
                    println!(
                        "live2d: loaded {} ({} motions in {} groups, {} expressions)",
                        self.model_path.display(),
                        scene.motion_count(),
                        scene.motion_groups().count(),
                        scene.expression_count(),
                    );
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

        let advance_start = pipeline.profile.then(Instant::now);
        let advance_result = scene.advance(device, queue, delta);

        if let Some(start) = advance_start {
            pipeline.advance_nanos += start.elapsed().as_nanos();
        }

        if let Err(error) = advance_result {
            eprintln!("live2d: advance failed: {error}");
        }
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

        pipeline.renders.fetch_add(1, Ordering::Relaxed);

        if let Err(error) = scene.encode(encoder, target, clip) {
            eprintln!("live2d: encode failed: {error}");
        }
    }
}
