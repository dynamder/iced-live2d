//! Sweeps the look-at target at 60 fps and reports what moves, frame by frame.
//!
//! Written to catch discontinuities: the cursor crossing the model's centre line
//! is `AngleX` crossing its default value (0), which is where a wrong
//! normalisation branch, a dead zone or a stepped motion curve shows up as a
//! visible jolt. See `docs/mocari-suspected-physics-chaining.md`.
//!
//! cargo run -p live2d-render --example sweep -- <model.model3.json> [frames]
//!
//! Knobs, all optional (the full list is in `README.md`):
//!
//! - `SWEEP_DIRECT=<ParamId>` write that parameter directly instead of driving
//!   the look-at target, in `-1.0..=1.0`
//! - `SWEEP_RANGE=<lo>,<hi>` input range, default `-1,1`. `lo == hi` holds the
//!   input still, which separates "jumps with position" from "jumps with time".
//! - `SWEEP_ALL=1` watch every parameter instead of the six printed by default
//! - `SWEEP_JUMP=<f32>` report per-frame changes above this, default `0.02`
//! - `SWEEP_SETTLE=<frames>` frames to let physics settle before recording,
//!   default `60`; raise it when a swinging part drowns the jolt you are hunting
//! - `SWEEP_MOTION=1` keep the idle motion playing, which is the app's normal
//!   state; by default the sweep stops it to isolate physics

use live2d_render::wgpu;
use live2d_render::{Live2dScene, SceneOptions};

#[path = "shared/gpu.rs"]
mod gpu;
#[path = "shared/knobs.rs"]
mod knobs;

const TRACKED: &[&str] = &[
    "AngleX",
    "ParamEyeBallX",
    "ParamAngleX",
    "ParamBodyAngleX",
    "HipAngleX",
    "HipZ1",
    "FrontHairX1",
];

fn drive(scene: &mut Live2dScene, direct: &Option<String>, x: f32) {
    match direct {
        Some(id) => scene.set_parameter_offset(id, x),
        None => scene.look_at([x, 0.0]),
    }
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

    let gpu::Gpu { device, queue } = gpu::open()?;

    let mut scene = Live2dScene::load_with_options(
        &device,
        &queue,
        &model_path,
        wgpu::TextureFormat::Rgba8Unorm,
        SceneOptions {
            texture_scale: 0.25,
            ..SceneOptions::default()
        },
    )?;
    scene.set_look_at_mode(knobs::look_at_mode()?);

    let direct = std::env::var("SWEEP_DIRECT").ok();

    if !env_flag("SWEEP_MOTION") {
        scene.stop_motion();
    }

    if env_flag("LIVE2D_NO_PHYSICS") {
        scene.set_physics_enabled(false);
    }

    // Let physics settle at the left end before the first recorded frame.
    for _ in 0..settle {
        drive(&mut scene, &direct, low);
        scene.advance(&device, &queue, live2d_render::STEP)?;
    }

    report_issues(&mut scene);

    let watched: Vec<String> = if watch_all {
        scene.parameter_ids().to_vec()
    } else {
        TRACKED.iter().map(|id| (*id).to_owned()).collect()
    };

    if !watch_all {
        println!("frame\tx\t{}", TRACKED.join("\t"));
    }

    let mut previous: Option<Vec<f32>> = None;

    for frame in 0..frames {
        let t = frame as f32 / (frames.max(2) - 1) as f32;
        let x = low + (high - low) * t;

        drive(&mut scene, &direct, x);
        scene.advance(&device, &queue, live2d_render::STEP)?;

        let values: Vec<f32> = watched
            .iter()
            .map(|id| scene.parameter_value(id).unwrap_or(f32::NAN))
            .collect();

        if !watch_all {
            println!(
                "{frame}\t{x:.3}\t{}",
                values
                    .iter()
                    .map(|v| format!("{v:.3}"))
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
                    let delta = (after - before).abs();

                    (delta > jump_threshold).then_some((delta, id.as_str(), *before, *after))
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

fn report_issues(scene: &mut Live2dScene) {
    for issue in scene.take_issues() {
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
