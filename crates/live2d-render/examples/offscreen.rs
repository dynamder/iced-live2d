//! Renders one Live2D frame headlessly and writes it to a PNG, bypassing the window
//! compositor: the honest way to check framing, clipping and colour.
//!
//! cargo run -p live2d-render --example offscreen -- <model.model3.json> [out.png] [size] [steps]
//!
//! A capture must be reproducible from the command line alone, so all state comes
//! from the `LIVE2D_*` knobs in `README.md`.

use live2d_render::wgpu;
use live2d_render::{AssetOptions, ClipRect, Fade, LookAtMode};

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

    // Validate before the GPU opens: a typo then fails in a millisecond.
    let knobs = Knobs::from_env()?;

    let gpu::Gpu { device, queue } = gpu::open()?;

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut session = Session::load(
        &device,
        &queue,
        &model_path,
        format,
        (size, size),
        AssetOptions {
            texture_scale: knobs.texture_scale,
            scan_sidecars: true,
        },
        live2d_render::ModelOptions {
            mask_size: knobs.mask_size,
        },
    )?;

    session.model().set_look_at_mode(knobs.look_at_mode);

    if let Some(fill) = knobs.fill {
        session.set_fill(&queue, fill);
    }

    knobs.apply(&mut session);

    // Let physics and the motion settle, so the frame is not the rest pose.
    for _ in 0..steps {
        session.advance(&device, &queue, frame_time)?;

        if knobs.reclip {
            session.rebuild_clipping(&device, &queue);
        }
    }

    report_issues(&mut session);

    if let Some(point) = knobs.hit {
        match session.hit_area_at(point) {
            Some(name) => println!("hit at {point:?}: {name}"),
            None => println!("hit at {point:?}: nothing"),
        }
    }

    let target = Target::new(&device, size, format);

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

    let rgba = render::read_back(&device, &queue, encoder, &target)?;

    write_png(&out_path, size, &rgba)?;
    println!("wrote {out_path} ({size}x{size})");

    Ok(())
}

fn report_issues(session: &mut Session) {
    for issue in session.take_issues() {
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
    motion: Option<(usize, usize)>,
    expression: Option<usize>,
    clear_expressions: bool,
    no_motion: bool,
    fade: Option<Fade>,
    no_physics: bool,
    reclip: bool,
    look_at: Option<[f32; 2]>,
    look_at_mode: LookAtMode,
    fill: Option<f32>,
    hit: Option<[f32; 2]>,
}

impl Knobs {
    fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            texture_scale: env_f32("LIVE2D_TEXTURE_SCALE")?.unwrap_or(1.0),
            mask_size: env_index("LIVE2D_MASK")?.unwrap_or(1024) as u32,
            motion: env_pair("LIVE2D_MOTION")?,
            expression: env_index("LIVE2D_EXPR")?,
            clear_expressions: env_flag("LIVE2D_CLEAR_EXPR"),
            no_motion: env_flag("LIVE2D_NO_MOTION"),
            fade: env_fade()?,
            no_physics: env_flag("LIVE2D_NO_PHYSICS"),
            reclip: env_flag("LIVE2D_RECLIP"),
            look_at: env_look_at()?,
            look_at_mode: knobs::look_at_mode()?,
            fill: env_f32("LIVE2D_FILL")?,
            hit: env_pair_f32("LIVE2D_HIT")?,
        })
    }

    fn apply(&self, session: &mut Session) {
        // The fade is resolved when a motion starts, so it has to be set first.
        if let Some(fade) = self.fade {
            session.model().set_fade(Some(fade));
        }

        if let Some((group, clip)) = self.motion {
            session.model().play_motion(group, clip);
        }

        if let Some(expression) = self.expression {
            session.model().play_expression(expression);
        }

        if self.clear_expressions {
            session.model().clear_expressions();
        }

        if self.no_motion {
            session.model().stop_motion();
        }

        if self.no_physics {
            session.model().set_physics_enabled(false);
        }

        if let Some(target) = self.look_at {
            session.model().set_look_at(Some(target));
        }
    }
}

/// `LIVE2D_FADE`: seconds, or `cut` for none. Absent leaves the files to decide.
fn env_fade() -> Result<Option<Fade>, Box<dyn std::error::Error>> {
    let Some(raw) = env_string("LIVE2D_FADE")? else {
        return Ok(None);
    };

    if raw == "cut" {
        return Ok(Some(Fade::zero()));
    }

    let seconds: f32 = raw
        .parse()
        .map_err(|_| format!("LIVE2D_FADE must be seconds or \"cut\", got {raw:?}"))?;

    Ok(Some(Fade::from(std::time::Duration::from_secs_f32(
        seconds,
    ))))
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

/// Two non-negative integers, `group,clip`.
fn env_pair(name: &str) -> Result<Option<(usize, usize)>, Box<dyn std::error::Error>> {
    let Some(raw) = env_string(name)? else {
        return Ok(None);
    };

    let parts: Vec<usize> = raw
        .split(',')
        .map(|part| part.trim().parse())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("{name} must be `group,clip`, got {raw:?}"))?;

    match parts[..] {
        [group, clip] => Ok(Some((group, clip))),
        _ => Err(format!("{name} needs exactly two numbers, got {raw:?}").into()),
    }
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

fn env_look_at() -> Result<Option<[f32; 2]>, Box<dyn std::error::Error>> {
    env_pair_f32("LIVE2D_LOOK_AT")
}

/// Two floats, `x,y`.
fn env_pair_f32(name: &str) -> Result<Option<[f32; 2]>, Box<dyn std::error::Error>> {
    let Some(raw) = env_string(name)? else {
        return Ok(None);
    };

    let parts: Vec<f32> = raw
        .split(',')
        .map(|part| part.trim().parse())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("{name} must be two numbers, got {raw:?}"))?;

    match parts[..] {
        [x, y] => Ok(Some([x, y])),
        _ => Err(format!("{name} needs exactly two numbers, got {raw:?}").into()),
    }
}
