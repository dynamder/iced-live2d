//! Where a LookAt target comes from each frame.

use iced::Rectangle;
use iced::mouse;

/// Maps a cursor position and the widget's bounds to a LookAt target.
///
/// Answers `-1.0..=1.0` on both axes (`+y` up), or `None` to fall back to
/// [`Live2dModel::look_at`](crate::Live2dModel::look_at) — or straight ahead if the
/// host never set one. Called once per frame; keep it cheap.
///
/// A `fn` and not a closure, so the widget stays [`Clone`] with no captured state.
pub type LookAtSource = fn(cursor: mouse::Cursor, bounds: Rectangle) -> Option<[f32; 2]>;

/// The cursor in `-1.0..=1.0` on both axes (`+y` up), or `None` outside `bounds`.
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

/// [`position`] as a [`LookAtSource`], for `.look_at(look_at::cursor)`.
///
/// `None` outside the widget is what lets the model return to
/// [`Live2dModel::look_at`](crate::Live2dModel::look_at) instead of snapping to
/// the last edge position.
pub fn cursor(cursor: mouse::Cursor, bounds: Rectangle) -> Option<[f32; 2]> {
    position(cursor, bounds)
}
