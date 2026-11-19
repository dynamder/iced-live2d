//! Reports what the *data* contains — physics settings, the parameters each motion
//! drives, what each expression touches — which separates "the renderer is broken"
//! from "this model is authored sparsely", two things that look alike on screen.
//!
//! cargo run -p live2d-render --example inspect -- <path/to/model.model3.json>

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use live2d_render::mocari::assets::load_model_runtime;
use live2d_render::mocari::expression::load_expression;
use live2d_render::mocari::json::{Physics3, PhysicsValueKind};
use live2d_render::mocari::motion::load_motion;
use live2d_render::ModelBounds;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: inspect <path/to/model.model3.json>")?;

    let model = load_model_runtime(&path)?;
    let runtime = model.runtime();
    let model_dir = model.model_dir().map(Path::to_path_buf);

    println!("== runtime ==");
    println!("drawables: {}", runtime.meshes().len());

    report_framing(runtime);

    let mut per_texture = BTreeMap::<i32, usize>::new();
    let mut masked = 0usize;
    let mut inverted = 0usize;
    let mut vertices = 0usize;

    for mesh in runtime.meshes() {
        *per_texture.entry(mesh.texture_index()).or_default() += 1;
        if !mesh.masks().is_empty() && mesh.masks().iter().all(|mask| *mask >= 0) {
            masked += 1;
            if mesh.is_inverted_mask() {
                inverted += 1;
            }
        }
        vertices += mesh.vertices().len();
    }

    let polygons: usize = runtime.meshes().iter().map(|m| m.indices().len() / 3).sum();
    let mut all_sets: BTreeSet<Vec<i32>> = BTreeSet::new();
    let mut visible_sets: BTreeSet<Vec<i32>> = BTreeSet::new();
    for mesh in runtime.meshes() {
        let masks: Vec<i32> = mesh.masks().iter().copied().filter(|m| *m >= 0).collect();
        if masks.is_empty() {
            continue;
        }
        all_sets.insert(masks.clone());
        if mesh.opacity() > 0.0 {
            visible_sets.insert(masks);
        }
    }

    println!("vertices: {vertices}, polygons: {polygons}");
    println!(
        "distinct mask sets: {} (visible drawables only: {})",
        all_sets.len(),
        visible_sets.len()
    );
    println!(
        "per-texture drawable counts: {:?}",
        per_texture
            .iter()
            .map(|(t, n)| format!("{t}:{n}"))
            .collect::<Vec<_>>()
    );
    println!(
        "masked drawables: {masked} (inverted {inverted}, normal {})",
        masked - inverted
    );
    println!("parameters: {}", runtime.parameter_ids().len());
    println!("parts: {}", runtime.part_ids().len());
    println!("physics loaded: {}", runtime.physics().is_some());
    println!("physics options: {:?}", runtime.physics_options());
    println!(
        "pose: {}",
        if runtime.model().pose().is_some() {
            "yes"
        } else {
            "no"
        }
    );

    if let (Some(dir), Some(file)) = (model_dir.as_deref(), runtime.model().physics()) {
        println!("\n== physics3 file: {file} ==");
        let source = std::fs::read_to_string(dir.join(file))?;
        let physics = Physics3::from_json_str(&source)?;
        println!(
            "version {} | settings {} | inputs {} | outputs {}",
            physics.version(),
            physics.settings().len(),
            physics.meta().total_input_count(),
            physics.meta().total_output_count(),
        );

        for setting in physics.settings() {
            let inputs: Vec<String> = setting
                .inputs()
                .iter()
                .map(|input| {
                    format!(
                        "{}{}{}",
                        input.source().id(),
                        axis(input.kind()),
                        if input.reflect() { "(reflect)" } else { "" }
                    )
                })
                .collect();
            let outputs: Vec<String> = setting
                .outputs()
                .iter()
                .map(|output| format!("{}{}", output.destination().id(), axis(output.kind())))
                .collect();

            println!(
                "  setting {:>2} | weight {:.1} | vertices {:>2} | {} -> {}",
                setting.id(),
                setting.normalization().position().default(),
                setting.vertices().len(),
                inputs.join(", "),
                outputs.join(", "),
            );
        }
    } else {
        println!("\n== no physics3 referenced by model3.json ==");
    }

    if let Ok(filter) = std::env::var("LIVE2D_PARAMS") {
        println!("\n== parameters matching '{filter}' ==");
        for info in runtime.parameter_infos() {
            let id = info.id();
            if filter.split(',').any(|needle| {
                id.to_ascii_lowercase()
                    .contains(&needle.trim().to_ascii_lowercase())
            }) {
                println!(
                    "  {:<28} min={:<8.2} max={:<8.2} default={:<8.2} value={:.3}",
                    id,
                    info.minimum(),
                    info.maximum(),
                    info.default(),
                    info.value(),
                );
            }
        }
    }

    if let Some(dir) = model_dir.as_deref() {
        println!("\n== motions ==");
        for path in scan(dir, ".motion3.json") {
            let label = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();

            let motion = load_motion(&path)?;
            let mut targets: BTreeSet<String> = BTreeSet::new();
            let mut part_curves = 0;

            for curve in motion.curves() {
                if curve.target() == "PartOpacity" {
                    part_curves += 1;
                }
                targets.insert(format!("{}/{}", curve.target(), curve.id()));
            }

            println!(
                "  {label}: {:.2}s, {} curves ({} part opacity), {} distinct targets",
                motion.meta().duration(),
                motion.curves().len(),
                part_curves,
                targets.len(),
            );

            for target in targets.iter().take(24) {
                println!("      {target}");
            }
            if targets.len() > 24 {
                println!("      ... {} more", targets.len() - 24);
            }
        }

        println!("\n== expressions ==");
        for path in scan(dir, ".exp3.json") {
            let label = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let expression = load_expression(&path)?;
            let ids: Vec<String> = expression
                .parameters()
                .iter()
                .map(|parameter| parameter.id().to_owned())
                .collect();
            println!("  {label}: {}", ids.join(", "));
        }
    }

    Ok(())
}

fn axis(kind: PhysicsValueKind) -> &'static str {
    match kind {
        PhysicsValueKind::X => ".X",
        PhysicsValueKind::Y => ".Y",
        PhysicsValueKind::Angle => ".Angle",
    }
}

/// How the artwork sits inside the canvas the view frames it by: a model that looks
/// small or off-centre is usually an artist leaving spare room in the `.moc3`
/// canvas box, not a viewer bug.
fn report_framing(runtime: &live2d_render::mocari::ModelRuntime) {
    let canvas = runtime.canvas();
    let Some(frame) = ModelBounds::from_canvas(&canvas) else {
        println!("\n== framing ==\ncanvas unusable, the scene falls back to drawable bounds");
        return;
    };
    let art = ModelBounds::from_drawables(runtime.meshes());

    let [frame_x, frame_y] = frame.center();
    println!("\n== framing ==");
    println!(
        "canvas: {:.2} x {:.2} units (aspect {:.2}), origin ({:.0}, {:.0}) px, ppu {:.0}, reverse-y {}",
        frame.width(),
        frame.height(),
        frame.width() / frame.height(),
        canvas.origin_x(),
        canvas.origin_y(),
        canvas.pixels_per_unit(),
        canvas.reverse_y_coordinate(),
    );
    println!("frame centre: ({frame_x:+.3}, {frame_y:+.3})");

    let Some(art) = art else {
        return;
    };

    let [art_x, art_y] = art.center();
    let offset_x = art_x - frame_x;
    let offset_y = art_y - frame_y;

    println!(
        "art bounds: centre ({art_x:+.3}, {art_y:+.3}), size {:.3} x {:.3}",
        art.width(),
        art.height(),
    );
    println!(
        "art fills {:.0}% x {:.0}% of the canvas, centre offset {:+.0}% x {:+.0}%",
        100.0 * art.width() / frame.width(),
        100.0 * art.height() / frame.height(),
        100.0 * offset_x / frame.width(),
        100.0 * offset_y / frame.height(),
    );

    // The widget's default fill is 0.925, i.e. 1.85 of NDC; the scene fits the
    // canvas, not the art, so a canvas with spare room shrinks the model.
    let view_fill = 1.85;
    let scale = (view_fill / frame.width()).min(view_fill / frame.height());
    let screen_width = 100.0 * art.width() * scale * 0.5;
    let screen_height = 100.0 * art.height() * scale * 0.5;

    println!(
        "on screen: {screen_width:.0}% x {screen_height:.0}% of a square window, centre {:+.0}% x {:+.0}% off centre",
        100.0 * offset_x * scale * 0.5,
        100.0 * offset_y * scale * 0.5,
    );
}

fn scan(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(dir, 0, suffix, &mut found);
    found.sort();
    found
}

fn collect(dir: &Path, depth: usize, suffix: &str, out: &mut Vec<PathBuf>) {
    if depth > 2 {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            collect(&path, depth + 1, suffix, out);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_ascii_lowercase().ends_with(suffix))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}
