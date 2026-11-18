//! Sweeps the gaze slowly and counts how many pixels change per frame.
//!
//! `sweep` watches parameters and can only see whole-parameter jumps. A part
//! popping, a mask switching or a drawable appearing changes a small number of
//! pixels without moving any centroid, so this measures what the eye actually
//! sees: the size of the frame-to-frame difference. On a smooth sweep it should
//! stay flat; a spike is a visible hitch.
//!
//! cargo run -p live2d-render --example jolt -- <model.model3.json> [frames]
//!
//! Knobs, all optional (the full list is in `README.md`):
//!
//! - `JOLT_RANGE=<lo>,<hi>` input range, default `-0.15,0.15`; `lo == hi` holds
//!   the input still, which separates "moves with position" from "moves with time"
//! - `JOLT_MOTION=1` keep the idle motion playing, the app's normal state
//! - `JOLT_SETTLE=<frames>` frames before recording, default `120`
//! - `JOLT_SIZE=<px>` capture size, default `640` — match the app's physical size
//! - `LIVE2D_NO_PHYSICS` — flag, disables physics evaluation

use std::time::Duration;

use live2d_render::wgpu;
use live2d_render::{ClipRect, Live2dScene, SceneOptions};

#[path = "shared/gpu.rs"]
mod gpu;
#[path = "shared/render.rs"]
mod render;

use render::Target;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model_path = args.next().ok_or("usage: jolt <model> [frames]")?;
    let frames: usize = args
        .next()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(120);

    let (low, high) = env_range("JOLT_RANGE")?.unwrap_or((-0.15, 0.15));
    let settle: usize = env_f32("JOLT_SETTLE")?.unwrap_or(120.0) as usize;
    let size: u32 = env_f32("JOLT_SIZE")?.unwrap_or(640.0) as u32;

    let gpu::Gpu { device, queue } = gpu::open()?;

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut scene = Live2dScene::load_with_options(
        &device,
        &queue,
        &model_path,
        format,
        SceneOptions {
            texture_scale: 0.5,
            ..SceneOptions::default()
        },
    )?;
    scene.resize(&queue, 1.0);

    if !env_flag("JOLT_MOTION") {
        scene.stop_motion();
    }

    if env_flag("LIVE2D_NO_PHYSICS") {
        scene.set_physics_enabled(false);
    }

    let target = Target::new(&device, size, format);

    for _ in 0..settle {
        scene.set_gaze([low, 0.0]);
        scene.advance(&device, &queue, Duration::from_millis(16))?;
    }

    report_issues(&mut scene);

    let mut previous: Option<Vec<u8>> = None;
    let mut history = Vec::with_capacity(frames);

    for frame in 0..frames {
        let t = frame as f32 / (frames.max(2) - 1) as f32;
        let x = low + (high - low) * t;

        scene.set_gaze([x, 0.0]);
        scene.advance(&device, &queue, Duration::from_millis(16))?;

        let rgba = capture(&device, &queue, &mut scene, &target, size)?;

        if let Some(previous) = previous.as_ref() {
            let change = difference(previous, &rgba, size);
            history.push(change.count);

            println!(
                "{frame:>3} x={x:+.3} changed={:>6} at ({:.0}, {:.0}) strongest={}",
                change.count, change.centroid_x, change.centroid_y, change.strongest
            );
        }

        previous = Some(rgba);
    }

    if !history.is_empty() {
        let mut sorted = history.clone();
        sorted.sort_unstable();
        let median = sorted[sorted.len() / 2];
        let peak = sorted[sorted.len() - 1];
        let ratio = peak as f32 / median.max(1) as f32;

        println!("changed pixels per frame: median {median}, peak {peak} ({ratio:.1}x median)");
    }

    Ok(())
}

fn capture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &mut Live2dScene,
    target: &Target,
    size: u32,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("jolt.encoder"),
    });

    {
        let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("jolt.clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }

    scene.encode(
        &mut encoder,
        &target.view,
        ClipRect {
            x: 0,
            y: 0,
            width: size,
            height: size,
        },
    )?;

    render::read_back(device, queue, encoder, target)
}

struct Change {
    count: usize,
    centroid_x: f32,
    centroid_y: f32,
    strongest: u32,
}

fn difference(previous: &[u8], current: &[u8], size: u32) -> Change {
    let mut count = 0usize;
    let mut sum_x = 0.0_f32;
    let mut sum_y = 0.0_f32;
    let mut strongest = 0u32;

    for y in 0..size {
        for x in 0..size {
            let index = ((y * size + x) * 4) as usize;
            let mut delta = 0u32;

            for channel in 0..3 {
                let before = i32::from(previous[index + channel]);
                let after = i32::from(current[index + channel]);
                delta += (after - before).unsigned_abs();
            }

            if delta <= 24 {
                continue;
            }

            count += 1;
            sum_x += x as f32;
            sum_y += y as f32;
            strongest = strongest.max(delta);
        }
    }

    let divisor = count.max(1) as f32;

    Change {
        count,
        centroid_x: sum_x / divisor,
        centroid_y: sum_y / divisor,
        strongest,
    }
}

fn report_issues(scene: &mut Live2dScene) {
    for issue in scene.take_issues() {
        eprintln!("{issue}");
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some()
}

fn env_f32(name: &str) -> Result<Option<f32>, Box<dyn std::error::Error>> {
    let Ok(raw) = std::env::var(name) else {
        return Ok(None);
    };

    Ok(Some(raw.parse().map_err(|_| {
        format!("{name} must be a number, got {raw:?}")
    })?))
}

fn env_range(name: &str) -> Result<Option<(f32, f32)>, Box<dyn std::error::Error>> {
    let Ok(raw) = std::env::var(name) else {
        return Ok(None);
    };

    let parts: Vec<f32> = raw
        .split(',')
        .map(|part| part.trim().parse())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("{name} must be two numbers, got {raw:?}"))?;

    match parts[..] {
        [low, high] => Ok(Some((low, high))),
        _ => Err(format!("{name} needs exactly two numbers, got {raw:?}").into()),
    }
}
