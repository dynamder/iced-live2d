//! Where a LookAt target comes from each frame.
//!
//! Cubism calls the "look at this point" input a **LookAt** target and models the
//! thing that produces it as an `ICubismLookTarget`. [`LookAtSource`] is that
//! interface: a function that answers "given this frame's cursor and this
//! widget's bounds, where should the model look?".

use iced::Rectangle;
use iced::mouse;

/// Produces a LookAt target for one frame.
///
/// Returns a point in `-1.0..=1.0` on both axes, `-1` = left/top and `+y` up, or
/// `None` to fall back to [`Live2dModel::look_at`](crate::Live2dModel::look_at)
/// — or, if the host never set one, to looking straight ahead.
/// Called once per frame while the widget is drawn, so it must be cheap and must
/// not allocate.
///
/// It is a plain function pointer rather than a closure: mapping a cursor position
/// needs no captured state, and a `fn` keeps the widget `Clone`-able and free of
/// ownership puzzles. A host with more elaborate rules (damping, one axis only, a
/// scripted timeline) should write the smoothed value into
/// [`Live2dModel::look_at`](crate::Live2dModel::look_at) as the cursor moves —
/// there the last write wins over the source only while the cursor is *outside*
/// the widget, so damping is a host-side state machine, not a source.
pub type LookAtSource = fn(cursor: mouse::Cursor, bounds: Rectangle) -> Option<[f32; 2]>;

/// The cursor, mapped onto the widget's clip space.
///
/// This is the one place the widget turns a window position into `-1.0..=1.0` on
/// both axes (`+y` up): the same value drives the LookAt target and the hit test,
/// so a click can never disagree with where the model is looking.
///
/// `None` while the cursor is outside `bounds` or the widget has no area.
pub fn position(cursor: mouse::Cursor, bounds: Rectangle) -> Option<[f32; 2]> {
    let position = cursor.position_in(bounds)?;

    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return None;
    }

    Some([
        (position.x / bounds.width) * 2.0 - 1.0,
        1.0 - (position.y / bounds.height) * 2.0,
    ])
}

/// [`position`] as a [`LookAtSource`].
///
/// `None` while the cursor is outside `bounds`, which is what lets the model
/// return to the target set with [`Live2dModel::look_at`](crate::Live2dModel::look_at)
/// instead of snapping to the last edge position.
///
/// Pass it straight to the widget: `live2d(&model).look_at(look_at::cursor)`.
pub fn cursor(cursor: mouse::Cursor, bounds: Rectangle) -> Option<[f32; 2]> {
    position(cursor, bounds)
}
