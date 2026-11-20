use iced_live2d::*;

fn main() {
    let asset = Live2dAsset::new("model.model3.json", AssetOptions::default());
    let model = Live2dModel::<Raw>::new(&asset, ModelOptions::default());

    let _ = live2d(&model)
        .gestures(Gestures::default())
        .on_click(|_| ())
        .on_event(|_| ());
}
