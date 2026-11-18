//! Shows which drawables a single parameter flips, with their texture, blend
//! mode and mask set.
//!
//! Written to answer "why does this outfit toggle draw a wireframe hand?" —
//! the answer lives in the drawable's metadata, not in the renderer.
//!
//! cargo run -p live2d-render --example switch -- <model.model3.json> <ParamId> <value0> <value1>

use live2d_render::mocari::assets::load_model_runtime;
use live2d_render::mocari::moc3::Moc3DrawableMesh;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: switch <model> <ParamId> <v0> <v1>")?;
    let parameter = args.next().ok_or("missing parameter id")?;
    let first: f32 = args.next().unwrap_or_else(|| "0".into()).parse()?;
    let second: f32 = args.next().unwrap_or_else(|| "1".into()).parse()?;

    let model = load_model_runtime(&path)?;
    let runtime = model.runtime();

    println!(
        "parameter '{parameter}' exists: {}",
        runtime.parameter_value(&parameter).is_some()
    );
    println!("drawables: {}", runtime.meshes().len());

    let low = sample(&path, &parameter, first)?;
    let high = sample(&path, &parameter, second)?;

    println!(
        "\nvisible drawables at {parameter} = {first}: {}",
        low.iter().filter(|d| d.visible).count()
    );
    println!(
        "visible drawables at {parameter} = {second}: {}",
        high.iter().filter(|d| d.visible).count()
    );

    println!("\n== drawables that appear at {second} ==");
    for (index, entry) in high.iter().enumerate() {
        if entry.visible && !low[index].visible {
            entry.report(index);
        }
    }

    println!("\n== drawables that disappear at {second} ==");
    for (index, entry) in high.iter().enumerate() {
        if !entry.visible && low[index].visible {
            entry.report(index);
        }
    }

    Ok(())
}

struct Entry {
    visible: bool,
    opacity: f32,
    texture: i32,
    blend: String,
    draw_order: f32,
    masks: Vec<i32>,
    vertices: usize,
    bounds: Option<[f32; 4]>,
    min_color: [f32; 3],
    screen_color: [f32; 3],
}

impl Entry {
    fn report(&self, index: usize) {
        println!(
            "  #{index:<4} tex={} blend={:<14} order={:<7} opacity={:.2} verts={:<4} masks={:?} bounds={:?} multiply={:?} screen={:?}",
            self.texture,
            self.blend,
            self.draw_order,
            self.opacity,
            self.vertices,
            self.masks,
            self.bounds.map(|b| [
                (b[0] * 100.0).round() / 100.0,
                (b[1] * 100.0).round() / 100.0,
                (b[2] * 100.0).round() / 100.0,
                (b[3] * 100.0).round() / 100.0
            ]),
            self.min_color,
            self.screen_color,
        );
    }
}

fn sample(
    path: &str,
    parameter: &str,
    value: f32,
) -> Result<Vec<Entry>, Box<dyn std::error::Error>> {
    let mut model = load_model_runtime(path)?;
    let runtime = model.runtime_mut();

    runtime.set_parameter_override_normalized(parameter, value);
    runtime.apply_parameter_overrides();

    if runtime.update_meshes().is_none() {
        return Err("update_meshes failed".into());
    }

    Ok(runtime.meshes().iter().map(entry).collect())
}

fn entry(mesh: &Moc3DrawableMesh) -> Entry {
    let mut bounds: Option<[f32; 4]> = None;

    for vertex in mesh.vertices() {
        let [x, y] = vertex.position();
        bounds = Some(match bounds {
            Some([min_x, min_y, max_x, max_y]) => {
                [min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)]
            }
            None => [x, y, x, y],
        });
    }

    Entry {
        visible: mesh.opacity() > 0.0 && !mesh.vertices().is_empty(),
        opacity: mesh.opacity(),
        texture: mesh.texture_index(),
        blend: format!("{:?}", mesh.blend_mode()),
        draw_order: mesh.draw_order(),
        masks: mesh.masks().to_vec(),
        vertices: mesh.vertices().len(),
        bounds,
        min_color: mesh.multiply_color(),
        screen_color: mesh.screen_color(),
    }
}
