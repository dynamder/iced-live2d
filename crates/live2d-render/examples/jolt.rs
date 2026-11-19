//! Sweeps the look-at target slowly and counts how many pixels change per frame.
//!
//! `sweep` only sees whole-parameter jumps; a part popping, a mask switching or a
//! drawable appearing moves few pixels and no centroid. On a smooth sweep this
//! should stay flat, and a spike is a visible hitch.
//!
//! cargo run -p live2d-render --example jolt -- <model.model3.json> [frames]
//!
//! Knobs (`JOLT_RANGE`, `JOLT_MOTION`, `JOLT_SETTLE`, `JOLT_SIZE`,
//! `LIVE2D_NO_PHYSICS`) are listed in `README.md`.

use live2d_render::wgpu;
use live2d_render::{AssetOptions, ClipRect, ModelOptions};

#[path = "shared/gpu.rs"]
mod gpu;
#[path = "shared/knobs.rs"]
mod knobs;
#[path = "shared/render.rs"]
mod render;
#[path = "shared/session.rs"]
mod session;

use render::Target;
use session::Session;

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
    let look_at_mode = knobs::look_at_mode()?;

    let gpu::Gpu { device, queue } = gpu::open()?;

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut session = Session::load(
        &device,
        &queue,
        &model_path,
        format,
        (size, size),
        AssetOptions {
            texture_scale: 0.5,
            scan_sidecars: true,
        },
        ModelOptions::default(),
    )?;

    session.model().set_look_at_mode(look_at_mode);

    if !env_flag("JOLT_MOTION") {
        session.model().stop_motion();
    }

    if env_flag("LIVE2D_NO_PHYSICS") {
        session.model().set_physics_enabled(false);
    }

    let target = Target::new(&device, size, format);

    for _ in 0..settle {
        session.model().set_look_at(Some([low, 0.0]));
        session.advance(&device, &queue, live2d_render::STEP)?;
    }

    report_issues(&mut session);

    let mut previous: Option<Vec<u8>> = None;
    let mut history = Vec::with_capacity(frames);

    for frame in 0..frames {
        let t = frame as f32 / (frames.max(2) - 1) as f32;
        let x = low + (high - low) * t;

        session.model().set_look_at(Some([x, 0.0]));
        session.advance(&device, &queue, live2d_render::STEP)?;

        let rgba = capture(&device, &queue, &session, &target, size)?;

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
    session: &Session,
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

    session.encode(
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

fn report_issues(session: &mut Session) {
    for issue in session.take_issues() {
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
