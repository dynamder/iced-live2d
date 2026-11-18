//! Dumps structural facts about a `.model3.json` so we can tell what a model
//! actually contains before blaming the renderer.
//!
//! cargo run -p live2d-render --example dump -- <path/to/model.model3.json>

use std::collections::BTreeSet;

use live2d_render::mocari::assets::load_model_runtime;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: dump <path/to/model.model3.json>")?;

    let model = load_model_runtime(&path)?;
    let runtime = model.runtime();

    println!("model dir: {:?}", model.model_dir());
    println!("textures: {}", model.textures().len());
    println!("drawables: {}", runtime.meshes().len());
    println!("parameters: {}", runtime.parameter_ids().len());

    let mut negative_masks = 0;
    let mut mask_sets: BTreeSet<Vec<i32>> = BTreeSet::new();
    let mut masked = 0;

    for (index, mesh) in runtime.meshes().iter().enumerate() {
        let masks = mesh.masks();
        mask_sets.insert(masks.to_vec());

        if masks.iter().any(|mask| *mask < 0) {
            negative_masks += 1;
        }

        if !masks.is_empty() {
            masked += 1;
        }

        if index < 8 {
            println!(
                "  #{index:<4} masks={masks:?} blend={:?} draw_order={} order={}",
                mesh.blend_mode(),
                mesh.draw_order(),
                mesh.render_order(),
            );
        }
    }

    println!("drawables with masks: {masked}");
    println!("drawables with a negative mask index: {negative_masks}");
    println!("distinct mask sets: {}", mask_sets.len());

    for set in mask_sets.iter().take(15) {
        println!("   {set:?}");
    }

    println!("motion groups:");
    for (group, references) in runtime.model().motions() {
        println!("  {group}: {} clip(s)", references.len());
    }

    println!("expressions: {}", runtime.model().expressions().len());

    let canvas = runtime.canvas();
    println!(
        "canvas: px_per_unit={} origin=({}, {}) size={}x{} reverse_y={}",
        canvas.pixels_per_unit(),
        canvas.origin_x(),
        canvas.origin_y(),
        canvas.width(),
        canvas.height(),
        canvas.reverse_y_coordinate(),
    );

    let mut visible_bounds: Option<[f32; 4]> = None;
    let mut visible_drawables = 0;

    for mesh in runtime.meshes() {
        if mesh.opacity() <= 0.0 {
            continue;
        }

        visible_drawables += 1;

        for vertex in mesh.vertices() {
            let [x, y] = vertex.position();
            visible_bounds = Some(match visible_bounds {
                Some([min_x, min_y, max_x, max_y]) => [
                    min_x.min(x),
                    min_y.min(y),
                    max_x.max(x),
                    max_y.max(y),
                ],
                None => [x, y, x, y],
            });
        }
    }

    println!("visible drawables: {visible_drawables}");

    if let Some([min_x, min_y, max_x, max_y]) = visible_bounds {
        println!(
            "visible bounds: x {min_x}..{max_x} (w {}), y {min_y}..{max_y} (h {})",
            max_x - min_x,
            max_y - min_y,
        );
    }

    Ok(())
}
