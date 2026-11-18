//! Renders one Live2D frame headlessly and writes it to a PNG.
//!
//! This bypasses the window compositor entirely, so it is the honest way to
//! check framing, clipping and colour without a screen capture in the loop — and
//! it is the base for the pixel-hash regression tests mentioned in
//! `docs/rust-iced-live2d-plan.md`.
//!
//! cargo run -p live2d-render --example offscreen -- <model.model3.json> [out.png] [size] [steps]

use std::time::Duration;

use live2d_render::wgpu;
use live2d_render::{ClipRect, Live2dScene, SceneOptions};

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

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))?;
    println!("adapter: {}", adapter.get_info().name);

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("offscreen.device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))?;

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let options = SceneOptions {
        texture_scale: env_f32("LIVE2D_TEXTURE_SCALE").unwrap_or(1.0),
        mask_size: env_usize("LIVE2D_MASK").map(|v| v as u32).unwrap_or(1024),
        ..SceneOptions::default()
    };
    let mut scene = Live2dScene::load_with_options(&device, &queue, &model_path, format, options)?;
    scene.resize(&queue, 1.0);

    // Deterministic state control for diagnostics.
    if let Some(index) = env_usize("LIVE2D_MOTION") {
        scene.play_motion(index);
    }
    if let Some(index) = env_usize("LIVE2D_EXPR") {
        scene.play_expression(index);
    }
    if env_usize("LIVE2D_CLEAR_EXPR").is_some() {
        scene.clear_expressions();
    }
    if env_usize("LIVE2D_NO_MOTION").is_some() {
        scene.stop_motion();
    }
    if env_usize("LIVE2D_NO_PHYSICS").is_some() {
        scene.set_physics_enabled(false);
    }
    if let Ok(gaze) = std::env::var("LIVE2D_GAZE") {
        let parts: Vec<f32> = gaze
            .split(',')
            .filter_map(|value| value.trim().parse().ok())
            .collect();
        if parts.len() == 2 {
            scene.set_gaze([parts[0], parts[1]]);
        }
    }
    let reclip_every_frame = env_usize("LIVE2D_RECLIP").is_some();

    // Let physics and the first motion settle a little so the frame is
    // representative instead of the rest pose.
    for _ in 0..steps {
        scene.advance(&device, &queue, frame_time)?;

        if reclip_every_frame {
            scene.force_clipping_rebuild(&device, &queue)?;
        }
    }

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

fn env_f32(name: &str) -> Option<f32> {
    std::env::var(name).ok()?.parse().ok()
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok()?.parse().ok()
}
