//! Sweeps the gaze horizontally at 60 fps and prints what physics does.
//!
//! Written to catch discontinuities: the cursor crossing the model's centre line
//! is `AngleX` crossing its default value (0), which is exactly where a wrong
//! normalisation branch shows up as a visible jolt.
//!
//! cargo run -p live2d-render --example sweep -- <model.model3.json> [frames]

use std::time::Duration;

use live2d_render::wgpu;
use live2d_render::{Live2dScene, SceneOptions};

const TRACKED: &[&str] = &[
    "AngleX",
    "ParamAngleX",
    "ParamBodyAngleX",
    "HipAngleX",
    "HipZ1",
    "FrontHairX1",
];

fn drive(scene: &mut Live2dScene, direct: &Option<String>, x: f32) {
    match direct {
        Some(id) => scene.set_parameter_offset(id, x),
        None => scene.set_gaze([x, 0.0]),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model_path = args.next().ok_or("usage: sweep <model> [frames]")?;
    let frames: usize = args.next().map(|v| v.parse()).transpose()?.unwrap_or(240);

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("sweep.device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))?;

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

    let direct = std::env::var("SWEEP_DIRECT").ok();

    scene.stop_motion();
    // Let physics settle at the left extreme before the sweep starts.
    for _ in 0..60 {
        drive(&mut scene, &direct, -1.0);
        scene.advance(&device, &queue, Duration::from_millis(16))?;
    }

    println!("frame\tangle_x\t{}", TRACKED.join("\t"));

    let mut previous: Option<Vec<f32>> = None;

    for frame in 0..frames {
        let t = frame as f32 / (frames.max(2) - 1) as f32;
        let x = t * 2.0 - 1.0;

        drive(&mut scene, &direct, x);
        scene.advance(&device, &queue, Duration::from_millis(16))?;

        let values: Vec<f32> = TRACKED
            .iter()
            .map(|id| scene.parameter_value(id).unwrap_or(f32::NAN))
            .collect();

        let jump = previous
            .as_ref()
            .map(|previous| {
                previous
                    .iter()
                    .zip(&values)
                    .map(|(a, b)| (b - a).abs())
                    .fold(0.0_f32, f32::max)
            })
            .unwrap_or(0.0);

        println!(
            "{frame}\t{x:.3}\t{}\t(jump {jump:.3})",
            values
                .iter()
                .map(|v| format!("{v:.3}"))
                .collect::<Vec<_>>()
                .join("\t")
        );

        previous = Some(values);
    }

    Ok(())
}
