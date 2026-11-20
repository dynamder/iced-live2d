//! Sweeps the look-at target at 60 fps and reports what moves, frame by frame.
//!
//! It hunts discontinuities: the cursor crossing the model's centre line is
//! `AngleX` crossing its default, where a wrong normalisation branch or a dead zone
//! shows up as a visible jolt.
//!
//! cargo run -p live2d-render --example sweep -- <model.model3.json> [frames]
//!
//! The columns are a fixed list of conventionally named parameters, so a rig that
//! spells them differently or lacks them entirely prints `-` in their place; that is
//! a property of the model, not a physics result.
//!
//! Knobs: `SWEEP_DIRECT=<ParamId>` writes that parameter instead of the look-at
//! target, `SWEEP_RANGE=<low>,<high>`, `SWEEP_ALL=1` watches every parameter,
//! `SWEEP_JUMP=<delta>`, `SWEEP_SETTLE=<frames>`, `SWEEP_MOTION=1` keeps the
//! motion playing.

use live2d_render::wgpu;
use live2d_render::{AssetOptions, ModelOptions, ParameterWrite};

#[path = "shared/gpu.rs"]
mod gpu;
#[path = "shared/knobs.rs"]
mod knobs;
#[path = "shared/session.rs"]
mod session;

use session::Session;

const TRACKED: &[&str] = &[
    "AngleX",
    "ParamEyeBallX",
    "ParamAngleX",
    "ParamBodyAngleX",
    "HipAngleX",
    "HipZ1",
    "FrontHairX1",
];

fn drive(session: &mut Session, direct: &Option<String>, x: f32) {
    let Some(id) = direct else {
        session.model().set_look_at(Some([x, 0.0]));
        return;
    };

    let info = session.info();
    let Some(index) = info.parameter(id) else {
        return;
    };

    let range = &info.parameters[index];
    let amount = (x.clamp(-1.0, 1.0) + 1.0) * 0.5;
    let raw = range.minimum + amount * (range.maximum - range.minimum);

    session
        .model()
        .set_parameters(&[(index, ParameterWrite::new(raw))]);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model_path = args.next().ok_or("usage: sweep <model> [frames]")?;
    let frames: usize = args
        .next()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(240);

    let (low, high) = env_range("SWEEP_RANGE")?.unwrap_or((-1.0, 1.0));
    let watch_all = env_flag("SWEEP_ALL");
    let jump_threshold = env_f32("SWEEP_JUMP")?.unwrap_or(0.02);
    let settle: usize = env_f32("SWEEP_SETTLE")?.unwrap_or(60.0) as usize;
    let look_at_mode = knobs::look_at_mode()?;

    let gpu::Gpu { device, queue } = gpu::open()?;

    let mut session = Session::load(
        &device,
        &queue,
        &model_path,
        wgpu::TextureFormat::Rgba8Unorm,
        (1, 1),
        AssetOptions {
            texture_scale: 0.25,
            scan_sidecars: true,
        },
        ModelOptions::default(),
    )?;

    session.model().set_look_at_mode(look_at_mode);

    let direct = std::env::var("SWEEP_DIRECT").ok();

    if !env_flag("SWEEP_MOTION") {
        session.model().stop_motion();
    }

    if env_flag("LIVE2D_NO_PHYSICS") {
        session.model().set_physics_enabled(false);
    }

    // Let physics settle at the left end before the first recorded frame.
    for _ in 0..settle {
        drive(&mut session, &direct, low);
        session.advance(&device, &queue, live2d_render::STEP)?;
    }

    report_issues(&mut session);

    let watched: Vec<String> = if watch_all {
        session
            .info()
            .parameters
            .iter()
            .map(|parameter| parameter.name.to_string())
            .collect()
    } else {
        TRACKED.iter().map(|id| (*id).to_owned()).collect()
    };

    if !watch_all {
        println!("frame\tx\t{}", TRACKED.join("\t"));
    }

    let mut previous: Option<Vec<Option<f32>>> = None;

    for frame in 0..frames {
        let t = frame as f32 / (frames.max(2) - 1) as f32;
        let x = low + (high - low) * t;

        drive(&mut session, &direct, x);
        session.advance(&device, &queue, live2d_render::STEP)?;

        let values: Vec<Option<f32>> = watched
            .iter()
            .map(|id| session.model().parameter_value(id))
            .collect();

        if !watch_all {
            println!(
                "{frame}\t{x:.3}\t{}",
                values
                    .iter()
                    .map(|value| match value {
                        Some(value) => format!("{value:.3}"),
                        None => "-".to_owned(),
                    })
                    .collect::<Vec<_>>()
                    .join("\t")
            );
        }

        if let Some(previous) = previous.as_ref() {
            let mut jumps: Vec<(f32, &str, f32, f32)> = previous
                .iter()
                .zip(&values)
                .zip(&watched)
                .filter_map(|((before, after), id)| {
                    let (before, after) = ((*before)?, (*after)?);
                    let delta = (after - before).abs();

                    (delta > jump_threshold).then_some((delta, id.as_str(), before, after))
                })
                .collect();

            jumps.sort_by(|a, b| b.0.total_cmp(&a.0));

            for (delta, id, before, after) in jumps.iter().take(4) {
                println!(
                    "  jump @ frame {frame:>3} x={x:+.4}  {id:<20} {before:+.3} -> {after:+.3}  ({delta:.3})"
                );
            }
        }

        previous = Some(values);
    }

    Ok(())
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
    let Some(raw) = std::env::var(name).ok() else {
        return Ok(None);
    };

    Ok(Some(raw.parse().map_err(|_| {
        format!("{name} must be a number, got {raw:?}")
    })?))
}

fn env_range(name: &str) -> Result<Option<(f32, f32)>, Box<dyn std::error::Error>> {
    let Some(raw) = std::env::var(name).ok() else {
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
