//! The vocabulary input is reported in.
//!
//! Every payload carries the [`Live2dId`] of the model it happened on, so a host
//! driving several models never has to guess. The generic parameter is always the
//! interface's hit-area type — `Pointing<FaceHitArea>` — which is the only part of
//! a vocabulary that input mentions.

use iced::Point;
use iced::mouse;
use iced::touch;

use crate::model::Live2dId;

/// Which pointer did something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pointer {
    /// A mouse button.
    Mouse(mouse::Button),
    /// One finger on a touch screen.
    Touch(touch::Finger),
}

/// Where a pointer was, in the three spaces a host may need.
///
/// All three describe the same point: a widget-local pixel position, its clip-space
/// image (`-1.0..=1.0`, `+y` up) and its model-space image, which is what hit areas
/// and physics use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Widget-local pixels, origin at the widget's top-left corner.
    pub pixels: Point,
    /// `-1.0..=1.0` on both axes, `+y` up. Independent of zoom and framing.
    pub clip: [f32; 2],
    /// Model space: the space [`ModelInfo::bounds`](live2d_render::ModelInfo) is in.
    pub model: [f32; 2],
}

/// A button going down or up.
#[derive(Debug, Clone, PartialEq)]
pub struct Pointing<A> {
    /// Which model.
    pub model: Live2dId,
    /// Which pointer.
    pub pointer: Pointer,
    /// The hit area under the pointer, if there was one.
    pub area: Option<A>,
    /// Where it happened.
    pub at: Sample,
}

/// The pointer entering or leaving a hit area.
#[derive(Debug, Clone, PartialEq)]
pub struct Hovering<A> {
    /// Which model.
    pub model: Live2dId,
    /// The area involved.
    pub area: A,
    /// Where the pointer was.
    pub at: Sample,
}

/// The pointer moving over the widget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moving {
    /// Which model.
    pub model: Live2dId,
    /// Where the pointer is.
    pub at: Sample,
}

/// A pointer moving while held down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dragging {
    /// Which model.
    pub model: Live2dId,
    /// Which pointer.
    pub pointer: Pointer,
    /// Where the press happened.
    pub origin: Sample,
    /// Where the pointer is now.
    pub at: Sample,
}

impl Dragging {
    /// Distance travelled since the press, in model units.
    pub fn delta(&self) -> [f32; 2] {
        [
            self.at.model[0] - self.origin.model[0],
            self.at.model[1] - self.origin.model[1],
        ]
    }
}

/// The wheel or touchpad turned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scrolling {
    /// Which model.
    pub model: Live2dId,
    /// Lines scrolled, `+y` away from the user.
    pub lines: [f32; 2],
    /// Where the pointer was.
    pub at: Sample,
}

/// A press that will never be released: the window lost focus, or the touch was
/// taken away.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cancelled {
    /// Which model.
    pub model: Live2dId,
    /// Which pointer.
    pub pointer: Pointer,
}

/// What a pointer did, as the widget saw it.
#[derive(Debug, Clone, PartialEq)]
pub enum PointerEvent<A> {
    /// A button or finger went down.
    Pressed(Pointing<A>),
    /// A button or finger came up. Reported wherever the pointer is, inside the
    /// widget or not.
    Released(Pointing<A>),
    /// The press was cancelled; nothing will be released.
    Cancelled(Cancelled),
    /// The pointer moved. Only reported when the host asks for it.
    Moved(Moving),
    /// The pointer came onto a hit area.
    Entered(Hovering<A>),
    /// The pointer left a hit area.
    Exited(Hovering<A>),
    /// The pointer moved while held down. Only the press's hit area is reported.
    Dragged(Dragging),
    /// The wheel turned.
    Scrolled(Scrolling),
}

impl<A> PointerEvent<A> {
    /// Which model this happened on.
    pub fn model(&self) -> Live2dId {
        match self {
            PointerEvent::Pressed(pointed) | PointerEvent::Released(pointed) => pointed.model,
            PointerEvent::Cancelled(cancelled) => cancelled.model,
            PointerEvent::Moved(moved) => moved.model,
            PointerEvent::Entered(hovered) | PointerEvent::Exited(hovered) => hovered.model,
            PointerEvent::Dragged(dragged) => dragged.model,
            PointerEvent::Scrolled(scrolled) => scrolled.model,
        }
    }
}
