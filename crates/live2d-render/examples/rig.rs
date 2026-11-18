//! Prints how cursor tracking would be routed for a model, before you trust it.
//!
//! The routing is inferred from the model's physics wiring, so a new model can be
//! inspected in a second — no GPU, no window — instead of by watching the render
//! and guessing which convention the rig follows.
//!
//! cargo run -p live2d-render --example rig -- <model.model3.json>

use std::path::Path;

use live2d_render::mocari::assets::load_model_runtime;
use live2d_render::{ModelFacts, Rig, TrackingMode, resolve};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: rig <path/to/model.model3.json>")?;

    let loaded = load_model_runtime(&path)?;
    let runtime = loaded.runtime();
    let model_dir = loaded.model_dir();

    let physics_outputs = physics_outputs(runtime, model_dir);
    let facts = ModelFacts::new(runtime.parameter_ids().to_vec(), physics_outputs);

    println!("model: {path}");
    println!("parameters: {}", runtime.parameter_ids().len());

    for mode in [TrackingMode::Adapted, TrackingMode::ViewerDefault] {
        let rig = resolve(mode, &facts);
        println!("\n== {mode} ==");
        print_rig(&rig);

        if mode == TrackingMode::Adapted {
            let unbound: Vec<&str> = live2d_render::Slot::ALL
                .iter()
                .filter(|slot| rig.binding(**slot).is_none())
                .map(|slot| slot.name())
                .collect();

            if !unbound.is_empty() {
                println!("  not driven: {}", unbound.join(", "));
            }
        }
    }

    Ok(())
}

fn print_rig(rig: &Rig) {
    for binding in rig.bindings() {
        println!(
            "  {:<11} -> {:<18} {:<16} ({})",
            binding.slot.name(),
            binding.parameter,
            binding.route.to_string(),
            binding.source
        );
    }
}

/// Same rule as `Live2dScene`: every `Output.Destination.Id` in `physics3.json`.
fn physics_outputs(
    runtime: &live2d_render::mocari::ModelRuntime,
    model_dir: Option<&Path>,
) -> Vec<String> {
    let (Some(dir), Some(file)) = (model_dir, runtime.model().physics()) else {
        return Vec::new();
    };

    let Ok(source) = std::fs::read_to_string(dir.join(file)) else {
        return Vec::new();
    };

    let Ok(physics) = live2d_render::mocari::json::Physics3::from_json_str(&source) else {
        return Vec::new();
    };

    physics
        .settings()
        .iter()
        .flat_map(|setting| setting.outputs())
        .map(|output| output.destination().id().to_owned())
        .collect()
}
