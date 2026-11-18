//! Renders one Live2D frame headlessly and writes it to a PNG.
//!
//! This bypasses the window compositor entirely, so it is the honest way to
//! check framing, clipping and colour without a screen capture in the loop — and
//! it is the base for the pixel-hash regression tests mentioned in
//! `docs/rust-iced-live2d-plan.md`.
//!
//! cargo run -p live2d-render --example offscreen -- <model.model3.json> [out.png] [size] [steps]
//!
//! A capture must be reproducible from the command line alone, so every piece of
//! state is set through the `LIVE2D_*` knobs documented in `README.md`.

use live2d_render::wgpu;
use live2d_render::{ClipRect, Live2dScene, SceneOptions};

#[path = "shared/gpu.rs"]
mod gpu;
#[path = "shared/knobs.rs"]
mod knobs;
#[path = "shared/render.rs"]
mod render;

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
    let frame_time = live2d_render::STEP;

    // Validated before the GPU opens, so a typo fails in a millisecond instead of
    // after uploading 592 MB of textures.
    let knobs = Knobs::from_env()?;

    let gpu::Gpu { device, queue } = gpu::open()?;

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let options = SceneOptions {
        texture_scale: knobs.texture_scale,
        mask_size: knobs.mask_size,
        tracking: knobs::tracking()?,
        ..SceneOptions::default()
    };

    let mut scene = Live2dScene::load_with_options(&device, &queue, &model_path, format, options)?;
    scene.resize(&queue, 1.0);

    knobs.apply(&mut scene);

    // Let physics and the first motion settle a little so the frame is
    // representative instead of the rest pose.
    for _ in 0..steps {
        scene.advance(&device, &queue, frame_time)?;

        if knobs.reclip {
            scene.force_clipping_rebuild(&device, &queue)?;
        }
    }

    report_issues(&mut scene);

    let target = render::Target::new(&device, size, format);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("offscreen.encoder"),
    });

    // Opaque background, exactly like a host clearing its surface.
    {
        let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("offscreen.clear"),
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

    let rgba = render::read_back(&device, &queue, encoder, &target)?;

    write_png(&out_path, size, &rgba)?;
    println!("wrote {out_path} ({size}x{size})");

    Ok(())
}

fn report_issues(scene: &mut Live2dScene) {
    for issue in scene.take_issues() {
        eprintln!("{issue}");
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

/// Everything the capture reads from the environment, validated up front.
struct Knobs {
    texture_scale: f32,
    mask_size: u32,
    motion: Option<usize>,
    expression: Option<usize>,
    clear_expressions: bool,
    no_motion: bool,
    no_physics: bool,
    reclip: bool,
    gaze: Option<[f32; 2]>,
}

impl Knobs {
    fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            texture_scale: env_f32("LIVE2D_TEXTURE_SCALE")?.unwrap_or(1.0),
            mask_size: env_index("LIVE2D_MASK")?.unwrap_or(1024) as u32,
            motion: env_index("LIVE2D_MOTION")?,
            expression: env_index("LIVE2D_EXPR")?,
            clear_expressions: env_flag("LIVE2D_CLEAR_EXPR"),
            no_motion: env_flag("LIVE2D_NO_MOTION"),
            no_physics: env_flag("LIVE2D_NO_PHYSICS"),
            reclip: env_flag("LIVE2D_RECLIP"),
            gaze: env_gaze()?,
        })
    }

    fn apply(&self, scene: &mut Live2dScene) {
        if let Some(index) = self.motion {
            scene.play_motion(index);
        }

        if let Some(index) = self.expression {
            scene.play_expression(index);
        }

        if self.clear_expressions {
            scene.clear_expressions();
        }

        if self.no_motion {
            scene.stop_motion();
        }

        if self.no_physics {
            scene.set_physics_enabled(false);
        }

        if let Some(gaze) = self.gaze {
            scene.set_gaze(gaze);
        }
    }
}

fn env_f32(name: &str) -> Result<Option<f32>, Box<dyn std::error::Error>> {
    let Some(raw) = env_string(name)? else {
        return Ok(None);
    };

    Ok(Some(raw.parse().map_err(|_| {
        format!("{name} must be a number, got {raw:?}")
    })?))
}

fn env_index(name: &str) -> Result<Option<usize>, Box<dyn std::error::Error>> {
    let Some(raw) = env_string(name)? else {
        return Ok(None);
    };

    Ok(Some(raw.parse().map_err(|_| {
        format!("{name} must be a non-negative integer, got {raw:?}")
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
