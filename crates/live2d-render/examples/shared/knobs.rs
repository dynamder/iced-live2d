//! Knobs shared by the examples.

use live2d_render::TrackingMode;

/// `LIVE2D_TRACK=viewer` switches to the Cubism Viewer's own mouse bindings;
/// `adapted` (the default) resolves the rig's conventions.
pub fn tracking() -> Result<TrackingMode, Box<dyn std::error::Error>> {
    match std::env::var("LIVE2D_TRACK") {
        Ok(raw) => Ok(raw.parse()?),
        Err(_) => Ok(TrackingMode::default()),
    }
}
