//! Which stage a parameter write reaches physics at.
//!
//! ```text
//! cargo run -p live2d-render --example policy-render -- \
//!     "path/to/model.model3.json" ParamMouthOpenY 0.8 auto
//! ```
//!
//! Steps a baseline and a written instance for a second, then lists the parameters
//! that differ. A write physics overwrites changes nothing; one placed before
//! physics also moves the outputs that read it.

use live2d_render::{Asset, AssetId, AssetOptions, ParameterStage, ParameterWrite, STEP};

const FRAMES: u32 = 60;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: policy <model> <Param> <value> <auto|before|after>")?;
    let name = args.next().ok_or("missing parameter name")?;
    let value: f32 = args.next().ok_or("missing value")?.parse()?;
    let stage = match args.next().as_deref().unwrap_or("auto") {
        "auto" => ParameterStage::Auto,
        "before" => ParameterStage::BeforePhysics,
        "after" => ParameterStage::AfterPhysics,
        other => return Err(format!("unknown stage {other:?}").into()),
    };

    let prepared = Asset::parse(AssetId::unique(), &path, AssetOptions::default())?;
    let info = prepared.info();
    let index = info
        .parameter(&name)
        .ok_or_else(|| format!("{name:?} is not a parameter of this model"))?;

    let mut plain = prepared.model();
    let mut written = prepared.model();
    written.set_parameters(&[(index, ParameterWrite::new(value).at(stage))]);

    for _ in 0..FRAMES {
        plain.advance(STEP);
        written.advance(STEP);
    }

    println!("write {name} = {value} at {stage:?}");

    for issue in written.take_issues() {
        println!("issue: {issue}");
    }

    let mut changed = 0;

    for (position, parameter) in info.parameters.iter().enumerate() {
        let Some(other) = info.parameter_at(position) else {
            continue;
        };

        let (Some(before), Some(after)) = (
            plain.parameter_value_by_index(other),
            written.parameter_value_by_index(other),
        ) else {
            continue;
        };

        if (after - before).abs() > 1e-6 {
            changed += 1;
            println!("  {}: {before:.4} -> {after:.4}", parameter.name);
        }
    }

    println!("{changed} parameter(s) differ from the baseline");

    Ok(())
}
