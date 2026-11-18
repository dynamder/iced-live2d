//! Renders one Live2D frame headlessly and writes it to a PNG.
//!
//! This bypasses the window compositor entirely, so it is the honest way to
//! check framing, clipping and colour without a screen capture in the loop — and
//! it is the base for the pixel-hash regression tests mentioned in
//! `docs/rust-iced-live2d-plan.md`.
//!
//! cargo run -p live2d-render --example offscreen -- <model.model3.json> [out.png] [size] [steps]
//!
//! Every knob below is optional and exists so a capture is reproducible from the
//! command line alone. The documented list lives in `README.md`.
//!
//! - `LIVE2D_TEXTURE_SCALE` f32, upload resolution multiplier (default 1.0)
//! - `LIVE2D_MASK` u32, mask render target edge (default 1024)
//! - `LIVE2D_MOTION`, `LIVE2D_EXPR` usize, motion / expression index to apply
//! - `LIVE2D_CLEAR_EXPR`, `LIVE2D_NO_MOTION`, `LIVE2D_NO_PHYSICS`,
//!   `LIVE2D_RECLIP` — flags, active when the variable is present
//! - `LIVE2D_GAZE` "x,y" in `-1.0..=1.0`
//! - `LIVE2D_PROFILE` — flag, prints per-frame CPU and upload cost

use std::time::Duration;

use live2d_render::wgpu;
use live2d_render::{ClipRect, Live2dScene, SceneOptions};

#[path = "shared/mod.rs"]
mod shared;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model_path = args
        .next()
        .ok_or("usage: offscreen <model.model3.json> [out.png] [size] [steps]")?;
    let out_path = args.next().unwrap_or_else(|| "frame.png".to_owned());
    let size: u32 = args
        .next()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(900);
    let steps: u32 = args
        .next()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(30);
    let frame_time = Duration::from_millis(16);

    let shared::Gpu { device, queue } = shared::open()?;

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let options = SceneOptions {
        texture_scale: env_f32("LIVE2D_TEXTURE_SCALE")?.unwrap_or(1.0),
        mask_size: env_u32("LIVE2D_MASK")?.unwrap_or(1024),
        ..SceneOptions::default()
    };

    let mut scene = Live2dScene::load_with_options(&device, &queue, &model_path, format, options)?;
    scene.resize(&queue, 1.0);

    if let Some(index) = env_usize("LIVE2D_MOTION")? {
        scene.play_motion(index);
    }
    if let Some(index) = env_usize("LIVE2D_EXPR")? {
        scene.play_expression(index);
    }
    if env_flag("LIVE2D_CLEAR_EXPR") {
        scene.clear_expressions();
    }
    if env_flag("LIVE2D_NO_MOTION") {
        scene.stop_motion();
    }
    if env_flag("LIVE2D_NO_PHYSICS") {
        scene.set_physics_enabled(false);
    }
    if let Some(gaze) = env_gaze()? {
        scene.set_gaze(gaze);
    }

    let reclip_every_frame = env_flag("LIVE2D_RECLIP");

    // Let physics and the first motion settle a little so the frame is
    // representative instead of the rest pose.
    for _ in 0..steps {
        scene.advance(&device, &queue, frame_time)?;

        if reclip_every_frame {
            scene.force_clipping_rebuild(&device, &queue)?;
        }
    }

    report_issues(&mut scene);

    let target = create_target(&device, size, format);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("offscreen.encoder"),
    });

    // Opaque background, exactly like a host clearing its surface.
    {
        let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("offscreen.clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
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
        &view,
        ClipRect {
            x: 0,
            y: 0,
            width: size,
            height: size,
        },
    )?;

    let readback = create_readback(&device, size);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback.buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(readback.padded_bytes_per_row),
                rows_per_image: Some(size),
            },
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );

    queue.submit([encoder.finish()]);

    let slice = readback.buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device.poll(wgpu::PollType::wait_indefinitely())?;
    receiver.recv()??;

    let mapped = slice.get_mapped_range()?;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);

    for row in 0..size {
        let start = (row * readback.padded_bytes_per_row) as usize;
        rgba.extend_from_slice(&mapped[start..start + (size * 4) as usize]);
    }

    drop(mapped);
    readback.buffer.unmap();

    write_png(&out_path, size, &rgba)?;
    println!("wrote {out_path} ({size}x{size})");

    Ok(())
}

fn report_issues(scene: &mut Live2dScene) {
    for issue in scene.take_issues() {
        eprintln!("{issue}");
    }
}

fn create_target(device: &wgpu::Device, size: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen.target"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

struct Readback {
    buffer: wgpu::Buffer,
    padded_bytes_per_row: u32,
}

fn create_readback(device: &wgpu::Device, size: u32) -> Readback {
    let unpadded_bytes_per_row = size * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("offscreen.readback"),
        size: u64::from(padded_bytes_per_row) * u64::from(size),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    Readback {
        buffer,
        padded_bytes_per_row,
    }
}

fn write_png(path: &str, size: u32, rgba: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::create(path)?;
    let writer = std::io::BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()?;
    Ok(())
}

/// Reads an optional `f32` from the environment, rejecting values that do not
/// parse instead of silently treating them as unset.
fn env_f32(name: &str) -> Result<Option<f32>, Box<dyn std::error::Error>> {
    let Some(raw) = env_string(name)? else {
        return Ok(None);
    };

    Ok(Some(raw.parse().map_err(|_| {
        format!("{name} must be a number, got {raw:?}")
    })?))
}

fn env_u32(name: &str) -> Result<Option<u32>, Box<dyn std::error::Error>> {
    let Some(raw) = env_string(name)? else {
        return Ok(None);
    };

    Ok(Some(raw.parse().map_err(|_| {
        format!("{name} must be a positive integer, got {raw:?}")
    })?))
}

fn env_usize(name: &str) -> Result<Option<usize>, Box<dyn std::error::Error>> {
    let Some(raw) = env_string(name)? else {
        return Ok(None);
    };

    Ok(Some(raw.parse().map_err(|_| {
        format!("{name} must be an index, got {raw:?}")
    })?))
}

/// A flag is on whenever the variable is present, whatever its value.
fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some()
}

fn env_string(name: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn env_gaze() -> Result<Option<[f32; 2]>, Box<dyn std::error::Error>> {
    let Some(raw) = env_string("LIVE2D_GAZE")? else {
        return Ok(None);
    };

    let parts: Vec<f32> = raw
        .split(',')
        .map(|part| part.trim().parse())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("LIVE2D_GAZE must be two numbers, got {raw:?}"))?;

    match parts[..] {
        [x, y] => Ok(Some([x, y])),
        _ => Err(format!("LIVE2D_GAZE needs exactly two numbers, got {raw:?}").into()),
    }
}
