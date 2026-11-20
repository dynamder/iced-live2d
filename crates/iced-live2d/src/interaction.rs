//! The vocabulary input is reported in.
//!
//! Every payload carries the [`Live2dId`] of the model it happened on, so a host
//! driving several models never has to guess. The generic parameter is always the
//! interface's hit-area type — `Pointing<FaceHitArea>` — which is the only part of
//! a vocabulary that input mentions.

use std::time::{Duration, Instant};

use iced::Point;
use iced::mouse;

use crate::model::Live2dId;

/// Which pointer did something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pointer {
    /// A mouse button. Touch is not supported yet.
    Mouse(mouse::Button),
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
    /// When it happened. Raw input carries the clock, so a host that classifies
    /// clicks and holds itself can measure them.
    pub when: Instant,
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
    /// When it happened.
    pub when: Instant,
}

/// The pointer moving over the widget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moving {
    /// Which model.
    pub model: Live2dId,
    /// Where the pointer is.
    pub at: Sample,
    /// When it happened.
    pub when: Instant,
}

/// A pointer moving while held down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dragging<A> {
    /// Which model.
    pub model: Live2dId,
    /// Which pointer.
    pub pointer: Pointer,
    /// The hit area the drag started on, if any.
    pub area: Option<A>,
    /// Where the press happened.
    pub origin: Sample,
    /// Where the pointer is now.
    pub at: Sample,
    /// When this frame's position was seen.
    pub when: Instant,
}

impl<A> Dragging<A> {
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
    /// When it happened.
    pub when: Instant,
}

/// A press that will never be released: the window lost focus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cancelled {
    /// Which model.
    pub model: Live2dId,
    /// Which pointer.
    pub pointer: Pointer,
    /// When the press was lost.
    pub when: Instant,
}

/// What a pointer did, as the widget saw it.
#[derive(Debug, Clone, PartialEq)]
pub enum PointerEvent<A> {
    /// A button went down.
    Pressed(Pointing<A>),
    /// A button came up. Reported wherever the pointer is, inside the widget or
    /// not.
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
    Dragged(Dragging<A>),
    /// The wheel turned.
    Scrolled(Scrolling),
}

impl<A> PointerEvent<A> {
    /// When this happened.
    pub fn when(&self) -> Instant {
        match self {
            PointerEvent::Pressed(pointing) | PointerEvent::Released(pointing) => pointing.when,
            PointerEvent::Cancelled(cancelled) => cancelled.when,
            PointerEvent::Moved(moved) => moved.when,
            PointerEvent::Entered(hovered) | PointerEvent::Exited(hovered) => hovered.when,
            PointerEvent::Dragged(dragged) => dragged.when,
            PointerEvent::Scrolled(scrolled) => scrolled.when,
        }
    }

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

/// What the pointer is doing over a widget, for
/// [`Live2d::cursor`](crate::Live2d::cursor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hover<'a, A> {
    /// The hit area under the pointer, if any.
    pub area: Option<&'a A>,
    /// Whether a press is being held.
    pub pressed: bool,
}

/// A press that was released without turning into a drag.
#[derive(Debug, Clone, PartialEq)]
pub struct Tap<A> {
    /// Which model.
    pub model: Live2dId,
    /// Which pointer.
    pub pointer: Pointer,
    /// The hit area that was pressed and released, if the press started on one.
    pub area: Option<A>,
    /// Where it was released.
    pub at: Sample,
    /// How long the press was held.
    pub held: Duration,
}

/// A press that travelled far enough to count as a drag.
#[derive(Debug, Clone, PartialEq)]
pub struct Drag<A> {
    /// Which model.
    pub model: Live2dId,
    /// Which pointer.
    pub pointer: Pointer,
    /// The hit area the drag started on, if any. Fixed for the whole drag.
    pub area: Option<A>,
    /// Where the press happened.
    pub origin: Sample,
    /// Where the pointer is now.
    pub at: Sample,
    /// Pixels per second, measured over the last frame of the drag. Zero until the
    /// pointer has moved at least once, and left at its last value when it stops.
    pub velocity: [f32; 2],
}

impl<A> Drag<A> {
    /// Distance travelled since the press, in model units.
    pub fn delta(&self) -> [f32; 2] {
        [
            self.at.model[0] - self.origin.model[0],
            self.at.model[1] - self.origin.model[1],
        ]
    }
}

/// What a pointer did, once the sequences above have been classified.
///
/// Produced only when the widget was given [`Gestures`]; the raw [`PointerEvent`]s
/// keep arriving either way.
#[derive(Debug, Clone, PartialEq)]
pub enum Gesture<A> {
    /// Pressed and released on the same hit area, in under
    /// [`Gestures::long_press`](crate::Gestures::long_press), without moving.
    Clicked(Tap<A>),
    /// Two clicks in quick succession on the same hit area. The first of the two is
    /// not reported separately, so this arrives instead of it, never after it.
    DoubleClicked(Tap<A>),
    /// Held without moving for at least [`Gestures::long_press`]. Reported while the
    /// pointer is still down, and the release that follows is not a click.
    LongPressed(Tap<A>),
    /// The press travelled past [`Gestures::drag_threshold`](crate::Gestures::drag_threshold).
    DragStarted(Drag<A>),
    /// The pointer moved while held down, once per frame.
    Dragged(Drag<A>),
    /// The drag ended; the pointer is no longer down.
    DragEnded(Drag<A>),
    /// The pointer came onto a hit area. Reported here because the classified
    /// vocabulary is the only input vocabulary a widget has when gestures are on.
    Entered(Hovering<A>),
    /// The pointer left a hit area.
    Exited(Hovering<A>),
    /// The pointer moved over the widget.
    Moved(Moving),
    /// The wheel turned over the widget.
    Scrolled(Scrolling),
}

impl<A> Gesture<A> {
    /// Which model this happened on.
    pub fn model(&self) -> Live2dId {
        match self {
            Gesture::Clicked(tap) | Gesture::DoubleClicked(tap) | Gesture::LongPressed(tap) => {
                tap.model
            }
            Gesture::DragStarted(drag) | Gesture::Dragged(drag) | Gesture::DragEnded(drag) => {
                drag.model
            }
            Gesture::Entered(hovered) | Gesture::Exited(hovered) => hovered.model,
            Gesture::Moved(moved) => moved.model,
            Gesture::Scrolled(scrolled) => scrolled.model,
        }
    }
}

/// How raw pointer input is classified into [`Gesture`]s.
///
/// The defaults are the ones a desktop pointer feels right with; every field is
/// meant to be overridden.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gestures {
    /// Pixels a press may travel before it becomes a drag.
    pub drag_threshold: f32,
    /// How long a press must hold still to become a long press.
    pub long_press: Duration,
    /// How long a click waits for a possible second one.
    ///
    /// A click and a double click are not a prefix code: at the first release there
    /// is no way to know which one it is. With `Some(window)` every click is held for
    /// up to `window` — a second click inside it is reported as
    /// [`Gesture::DoubleClicked`] and the first click is never reported at all, so a
    /// host that handles both never sees one action twice. The cost is that a single
    /// click is reported late by that much; `None` reports every click immediately,
    /// and then a double click arrives as two [`Gesture::Clicked`] events.
    pub double_click: Option<Duration>,
}

impl Default for Gestures {
    fn default() -> Self {
        Self {
            drag_threshold: 5.0,
            long_press: Duration::from_millis(500),
            double_click: Some(Duration::from_millis(300)),
        }
    }
}
