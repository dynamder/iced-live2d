//! Knobs shared by the examples.

use live2d_render::LookAtMode;

/// `LIVE2D_TRACK=viewer` uses the Cubism Viewer's mouse bindings; `adapted` (the
/// default) follows the rig's conventions.
pub fn look_at_mode() -> Result<LookAtMode, Box<dyn std::error::Error>> {
    match std::env::var("LIVE2D_TRACK") {
        Ok(raw) => Ok(raw.parse()?),
        Err(_) => Ok(LookAtMode::default()),
    }
}
