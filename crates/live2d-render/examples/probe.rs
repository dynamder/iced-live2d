//! Lists the drawable stack covering a single model-space point.
//!
//! Use it when a region of the render looks wrong: it shows exactly which
//! drawables are painted there, in paint order, with blend mode, opacity,
//! texture and mask set.
//!
//! cargo run -p live2d-render --example probe -- <model> <x> <y> [ParamId=value ...]

use live2d_render::mocari::assets::load_model_runtime;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: probe <model> <x> <y> [Param=value ...]")?;
    let x: f32 = args.next().ok_or("missing x")?.parse()?;
    let y: f32 = args.next().ok_or("missing y")?.parse()?;

    let overrides: Vec<(String, f32)> = args
        .map(|arg| {
            let (id, value) = arg.split_once('=').ok_or("override must be Param=value")?;
            Ok((id.to_owned(), value.parse::<f32>()?))
        })
        .collect::<Result<_, Box<dyn std::error::Error>>>()?;

    let mut model = load_model_runtime(&path)?;
    let runtime = model.runtime_mut();

    for (id, value) in &overrides {
        runtime.set_parameter_override_normalized(id, *value);
    }

    runtime.apply_parameter_overrides();

    if runtime.update_meshes().is_none() {
        return Err("update_meshes failed".into());
    }

    println!("point ({x}, {y}) with {overrides:?}");

    let mut stack: Vec<(i32, usize)> = Vec::new();

    for (index, mesh) in runtime.meshes().iter().enumerate() {
        if mesh.opacity() <= 0.0 || mesh.vertices().is_empty() {
            continue;
        }

        if !covers(mesh, x, y) {
            continue;
        }

        stack.push((mesh.render_order(), index));
    }

    stack.sort_unstable();

    println!("{} drawable(s) cover this point, in paint order:", stack.len());

    for (_, index) in stack {
        let mesh = &runtime.meshes()[index];
        println!(
            "  #{index:<4} order={:<5} tex={} blend={:<14} opacity={:.3} verts={:<4} masks={:?}",
            mesh.render_order(),
            mesh.texture_index(),
            format!("{:?}", mesh.blend_mode()),
            mesh.opacity(),
            mesh.vertices().len(),
            mesh.masks(),
        );
    }

    Ok(())
}

/// Coarse point-in-drawable test: inside the drawable's bounding box.
fn covers(mesh: &moc3_mesh::Mesh, x: f32, y: f32) -> bool {
    let mut min = [f32::MAX; 2];
    let mut max = [f32::MIN; 2];

    for vertex in mesh.vertices() {
        let [vx, vy] = vertex.position();
        min[0] = min[0].min(vx);
        min[1] = min[1].min(vy);
        max[0] = max[0].max(vx);
        max[1] = max[1].max(vy);
    }

    x >= min[0] && x <= max[0] && y >= min[1] && y <= max[1]
}

mod moc3_mesh {
    pub use live2d_render::mocari::moc3::Moc3DrawableMesh as Mesh;
}
