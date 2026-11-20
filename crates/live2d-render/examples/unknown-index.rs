//! What an out-of-range index does: every one is reported, none is applied.
//!
//! ```text
//! cargo run -p live2d-render --example unknown-index -- path/to/model.model3.json
//! ```
//!
//! Writes one parameter and one part the model does not declare, plays one
//! expression it does not have, then prints the issues the model reports. No GPU.

use live2d_render::{
    Asset, AssetId, AssetOptions, ExpressionIndex, ParameterIndex, ParameterWrite, PartIndex, STEP,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: unknown-index <model.model3.json>")?;

    let prepared = Asset::parse(AssetId::unique(), &path, AssetOptions::default())?;
    let info = prepared.info();
    let mut model = prepared.model();

    let parameter = ParameterIndex::new(info.parameters.len());
    let part = PartIndex::new(info.parts.len());
    let expression = ExpressionIndex::new(info.expressions.len());

    model.set_parameters(&[(parameter, ParameterWrite::new(1.0))]);
    model.set_parts(&[(part, 0.5)]);
    model.play_expression(expression);
    model.advance(STEP);

    println!("model: {path}");
    println!(
        "exposes {} parameters, {} parts, {} expressions",
        info.parameters.len(),
        info.parts.len(),
        info.expressions.len()
    );

    for issue in model.take_issues() {
        println!("  {issue}");
    }

    Ok(())
}
