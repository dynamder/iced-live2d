//! The widget.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use iced::advanced::layout;
use iced::advanced::widget::tree;
use iced::advanced::{Layout, Widget, renderer};
use iced::widget::shader;
use iced::{Length, Rectangle, mouse, touch, window};
use live2d_render::STEP;

use crate::event::{Live2dEvent, Problem};
use crate::gesture::Recognizer;
use crate::interaction::{
    Cancelled, Drag, Dragging, Gesture, Gestures, Hover, Hovering, Moving, Pointer, PointerEvent,
    Pointing, Scrolling, Tap,
};
use crate::interface::{Interface, KeyId, Vocabulary};
use crate::look_at::LookAtSource;
use crate::model::{Command, Live2dModel, PointerInput, Presence};
use crate::pipeline::{FrameInputs, Live2dPrimitive, PipelineEvent, PointerEventKind, ViewId};

/// `1.0` touches the edges; the sample models were framed for this.
const DEFAULT_FILL: f32 = 0.925;

/// Half a step: iced rounds a redraw deadline *up* to the refresh grid, so a full
/// step lands one refresh too late and halves the frame rate.
const REDRAW_LEAD: Duration = Duration::from_micros(STEP.as_micros() as u64 / 2);

/// Pixels a scroll notch carries, when the platform reports pixels.
const PIXELS_PER_LINE: f32 = 50.0;

/// Starts a model widget: `live2d(&self.hiyori).on_event(Message::Live2d)`.
///
/// The widget borrows the model, so the model must outlive the UI tree. Two
/// widgets may draw one model; each frames it and reports pointer input in its own
/// space, but its events reach both.
pub fn live2d<Message, I: Interface>(model: &Live2dModel<I>) -> Live2d<'_, Message, I, Unset> {
    Live2d::new(model)
}

/// Which input vocabulary a widget reports. Chosen once, at construction: the two
/// vocabularies are mutually exclusive, so one physical action can only ever be
/// handled once.
pub trait Mode: sealed::Mode + 'static {
    /// The input vocabulary this mode reports.
    type Input<A: Clone + std::fmt::Debug + 'static>: Clone + std::fmt::Debug + 'static;
}

/// Nothing chosen yet: the widget reports raw pointer events, and the raw hooks move
/// it into [`RawMode`].
pub struct Unset;

/// Raw pointer facts only, each with the instant it happened. A double click is two
/// of those, and telling them apart is the host's business.
pub struct RawMode;

/// Classified input only: clicks, double clicks, long presses, drags, hover, scroll.
/// No raw pointer event is delivered.
pub struct GestureMode;

mod sealed {
    pub trait Mode {}

    impl Mode for super::Unset {}
    impl Mode for super::RawMode {}
    impl Mode for super::GestureMode {}
}

impl Mode for Unset {
    type Input<A: Clone + std::fmt::Debug + 'static> = PointerEvent<A>;
}

impl Mode for RawMode {
    type Input<A: Clone + std::fmt::Debug + 'static> = PointerEvent<A>;
}

impl Mode for GestureMode {
    type Input<A: Clone + std::fmt::Debug + 'static> = Gesture<A>;
}

/// A Live2D model drawn as a widget. wgpu only; the tiny-skia backend cannot draw
/// iced's `shader` widget. Box it with `column![…]` or `container`.
///
/// The widget drives its own frames. A host must **not** also subscribe to
/// [`window::frames`]: `RedrawRequest` holds one request and the last writer wins.
///
/// Input reaches the host through the `on_*` methods, each of which takes a closure
/// that maps the event to the host's own message. Nothing is decided here: the
/// component reports what happened and the host decides what it means.
pub struct Live2d<'a, Message, I: Interface, M: Mode = Unset> {
    model: &'a Live2dModel<I>,
    handlers: Handlers<'a, Message, I, M>,
    gestures: Option<Gestures>,
    mode: PhantomData<M>,
    cursor: Option<Cursor<'a, I::HitArea>>,
    look_at: Option<LookAtSource>,
    capture: bool,
    fill: f32,
    profile: bool,
    width: Length,
    height: Length,
}

/// One mapping from an input to the host's own message.
type Handler<'a, Event, Message> = Rc<dyn Fn(Event) -> Message + 'a>;

/// The catch-all [`Live2d::on_event`] closure, whose event type follows the mode.
type AnyHandler<'a, Message, I, M> =
    Handler<'a, Live2dEvent<I, <M as Mode>::Input<<I as Interface>::HitArea>>, Message>;

/// The cursor hook: a pure function of hover, so it costs no message.
type Cursor<'a, A> = Rc<dyn Fn(Hover<'_, A>) -> mouse::Interaction + 'a>;

/// One closure per input the host asked for. A missing closure means "not
/// interested", including for the frame request that input needs.
struct Handlers<'a, Message, I: Interface, M: Mode> {
    any: Option<AnyHandler<'a, Message, I, M>>,
    press: Option<Handler<'a, Pointing<I::HitArea>, Message>>,
    release: Option<Handler<'a, Pointing<I::HitArea>, Message>>,
    click: Option<Handler<'a, Tap<I::HitArea>, Message>>,
    double_click: Option<Handler<'a, Tap<I::HitArea>, Message>>,
    long_press: Option<Handler<'a, Tap<I::HitArea>, Message>>,
    drag_start: Option<Handler<'a, Drag<I::HitArea>, Message>>,
    drag: Option<Handler<'a, Drag<I::HitArea>, Message>>,
    drag_end: Option<Handler<'a, Drag<I::HitArea>, Message>>,
    scroll: Option<Handler<'a, Scrolling, Message>>,
    enter: Option<Handler<'a, Hovering<I::HitArea>, Message>>,
    exit: Option<Handler<'a, Hovering<I::HitArea>, Message>>,
    moved: Option<Handler<'a, Moving, Message>>,
}

impl<Message, I: Interface, M: Mode> Default for Handlers<'_, Message, I, M> {
    fn default() -> Self {
        Self {
            any: None,
            press: None,
            release: None,
            click: None,
            double_click: None,
            long_press: None,
            drag_start: None,
            drag: None,
            drag_end: None,
            scroll: None,
            enter: None,
            exit: None,
            moved: None,
        }
    }
}

impl<Message, I: Interface, M: Mode> Clone for Handlers<'_, Message, I, M> {
    fn clone(&self) -> Self {
        Self {
            any: self.any.clone(),
            press: self.press.clone(),
            release: self.release.clone(),
            click: self.click.clone(),
            double_click: self.double_click.clone(),
            long_press: self.long_press.clone(),
            drag_start: self.drag_start.clone(),
            drag: self.drag.clone(),
            drag_end: self.drag_end.clone(),
            scroll: self.scroll.clone(),
            enter: self.enter.clone(),
            exit: self.exit.clone(),
            moved: self.moved.clone(),
        }
    }
}

impl<Message, I: Interface, M: Mode> Handlers<'_, Message, I, M> {
    /// Whether any input at all is wanted, which decides whether the widget asks
    /// the render layer for events.
    fn is_empty(&self) -> bool {
        self.any.is_none()
            && self.press.is_none()
            && self.release.is_none()
            && self.click.is_none()
            && self.double_click.is_none()
            && self.long_press.is_none()
            && self.drag_start.is_none()
            && self.drag.is_none()
            && self.drag_end.is_none()
            && self.scroll.is_none()
            && self.enter.is_none()
            && self.exit.is_none()
            && self.moved.is_none()
    }
}

impl<'a, Message, I: Interface, M: Mode> Live2d<'a, Message, I, M> {
    /// A widget for `model`, filling its container.
    pub fn new(model: &'a Live2dModel<I>) -> Self {
        Self {
            model,
            handlers: Handlers::default(),
            gestures: None,
            mode: PhantomData,
            cursor: None,
            look_at: None,
            capture: false,
            fill: DEFAULT_FILL,
            profile: false,
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    /// The same widget, reporting in another vocabulary.
    fn cast<M2: Mode>(self) -> Live2d<'a, Message, I, M2> {
        Live2d {
            model: self.model,
            handlers: Handlers::default(),
            gestures: self.gestures,
            mode: PhantomData,
            cursor: self.cursor,
            look_at: self.look_at,
            capture: self.capture,
            fill: self.fill,
            profile: self.profile,
            width: self.width,
            height: self.height,
        }
    }

    /// Sends every [`Live2dEvent`] to the host as `Message`.
    ///
    /// Without it events are dropped, load failures included. At most one per
    /// frame, one frame late; a burst is queued, not lost.
    ///
    /// A more specific `on_*` handler wins over this one for the events it covers.
    pub fn on_event(
        mut self,
        on_event: impl Fn(Live2dEvent<I, M::Input<I::HitArea>>) -> Message + 'a,
    ) -> Self {
        self.handlers.any = Some(Rc::new(on_event));
        self
    }

    /// The wheel turned over the widget.
    pub fn on_scroll(mut self, on_scroll: impl Fn(Scrolling) -> Message + 'a) -> Self {
        self.handlers.scroll = Some(Rc::new(on_scroll));
        self
    }

    /// The pointer came onto a hit area.
    pub fn on_enter(mut self, on_enter: impl Fn(Hovering<I::HitArea>) -> Message + 'a) -> Self {
        self.handlers.enter = Some(Rc::new(on_enter));
        self
    }

    /// The pointer left a hit area.
    pub fn on_exit(mut self, on_exit: impl Fn(Hovering<I::HitArea>) -> Message + 'a) -> Self {
        self.handlers.exit = Some(Rc::new(on_exit));
        self
    }

    /// The pointer moved over the widget, once per frame.
    ///
    /// Only reported when this is set: it is the one continuous input, and a host
    /// that does not need it should not pay a message per frame.
    pub fn on_move(mut self, on_move: impl Fn(Moving) -> Message + 'a) -> Self {
        self.handlers.moved = Some(Rc::new(on_move));
        self
    }

    /// Sets the cursor shape from what is under the pointer.
    ///
    /// Off by default, and a pure function of hover, so it costs no message:
    ///
    /// ```text
    /// .cursor(|hover| match hover {
    ///     Hover { pressed: true, .. } => mouse::Interaction::Grabbing,
    ///     Hover { area: Some(_), .. } => mouse::Interaction::Pointer,
    ///     _ => mouse::Interaction::Idle,
    /// })
    /// ```
    pub fn cursor(
        mut self,
        cursor: impl Fn(Hover<'_, I::HitArea>) -> mouse::Interaction + 'a,
    ) -> Self {
        self.cursor = Some(Rc::new(cursor));
        self
    }

    /// Captures pointer events while a press is held, so a parent widget that
    /// scrolls or drags does not also react. Off by default.
    pub fn capture(mut self, capture: bool) -> Self {
        self.capture = capture;
        self
    }

    /// Lets the cursor point the model's head, body and eyes: pass
    /// [`look_at::cursor`](crate::look_at::cursor) or a source of your own.
    ///
    /// Off by default. The source only wins while it answers; outside the widget
    /// the gaze falls back to [`Live2dModel::look_at`].
    pub fn look_at(mut self, source: LookAtSource) -> Self {
        self.look_at = Some(source);
        self
    }

    /// How much of the widget the model's bounds fill; defaults to `0.925`.
    ///
    /// A value that is not positive and finite falls back to `1.0`.
    pub fn fill(mut self, fill: f32) -> Self {
        self.fill = fill;
        self
    }

    /// Prints this widget's per-frame cost every 60 frames. Off by default.
    pub fn profile(mut self, profile: bool) -> Self {
        self.profile = profile;
        self
    }

    /// Sets the width, as any other iced widget does.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Sets the height, as any other iced widget does.
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Whether the render layer should report input to this widget at all.
    fn listens(&self) -> bool {
        !self.handlers.is_empty() || self.gestures.is_some() || self.cursor.is_some()
    }

    /// Queues the input a pointer event carries.
    ///
    /// Positions are captured here, not in `prepare`: iced delivers the event now
    /// and the render layer runs later, so a press that is followed by a fast move
    /// must remember where it landed. A release is queued wherever it happens,
    /// inside the widget or not.
    ///
    /// Returns whether the event needs a frame, which is also true while the model
    /// is paused: a frozen model still has a pose to hit-test.
    fn queue_input(
        &self,
        state: &mut Live2dState<I, M>,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        when: Instant,
    ) -> bool {
        let model = self.model.inner();

        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(button)) if cursor.is_over(bounds) => {
                state.pressed = true;

                if let Some(at) = cursor.position() {
                    model.push_pointer(PointerInput::Pressed {
                        pointer: Pointer::Mouse(*button),
                        at,
                        when,
                    });
                }
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(button)) => {
                state.pressed = false;

                if let Some(at) = cursor.position() {
                    model.push_pointer(PointerInput::Released {
                        pointer: Pointer::Mouse(*button),
                        at,
                        when,
                    });
                }
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let lines = match delta {
                    mouse::ScrollDelta::Lines { x, y } => [*x, *y],
                    mouse::ScrollDelta::Pixels { x, y } => {
                        [*x / PIXELS_PER_LINE, *y / PIXELS_PER_LINE]
                    }
                };

                if let Some(at) = cursor.position() {
                    model.push_pointer(PointerInput::Scrolled { lines, at, when });
                }
            }
            iced::Event::Touch(touch::Event::FingerPressed { id, position }) => {
                model.push_pointer(PointerInput::Pressed {
                    pointer: Pointer::Touch(*id),
                    at: *position,
                    when,
                });
            }
            iced::Event::Touch(touch::Event::FingerMoved { id, position }) => {
                model.push_pointer(PointerInput::Moved {
                    pointer: Pointer::Touch(*id),
                    at: *position,
                });
            }
            iced::Event::Touch(touch::Event::FingerLifted { id, position }) => {
                model.push_pointer(PointerInput::Released {
                    pointer: Pointer::Touch(*id),
                    at: *position,
                    when,
                });
            }
            iced::Event::Touch(touch::Event::FingerLost { id, .. }) => {
                model.push_pointer(PointerInput::Cancelled {
                    pointer: Pointer::Touch(*id),
                });
            }
            iced::Event::Window(window::Event::Unfocused) => {
                state.pressed = false;
                model.push_pointer(PointerInput::CancelAll);
            }
            _ => return false,
        }

        true
    }
}

impl<'a, Message, I: Interface, M: Mode> Live2d<'a, Message, I, M> {
    /// Turns a render-layer event into the host's, stamping the model identity.
    fn translate(
        &self,
        event: PipelineEvent,
    ) -> Result<Live2dEvent<I, M::Input<I::HitArea>>, Problem> {
        let model = self.model.id();

        Ok(match event {
            PipelineEvent::Loaded { info } => Live2dEvent::ModelLoaded { model, info },
            PipelineEvent::LoadFailed { failure } => {
                Live2dEvent::ModelFailedToLoad { model, failure }
            }
            PipelineEvent::Problem(problem) => Live2dEvent::ModelReportedProblem { model, problem },
            PipelineEvent::MotionStarted {
                group,
                clip,
                duration,
                looping,
            } => Live2dEvent::MotionStarted {
                model,
                group: key::<I::Motion>(group, "motion")?,
                clip,
                duration,
                looping,
            },
            PipelineEvent::MotionEnded { group, clip, ended } => Live2dEvent::MotionEnded {
                model,
                group: key::<I::Motion>(group, "motion")?,
                clip,
                ended,
            },
            PipelineEvent::Pointer(_) => {
                unreachable!("pointer events are turned into input before this point")
            }
        })
    }
}

/// The pointer facts that are not about a button, expressed in the classified
/// vocabulary: hover, movement and the wheel have no click to arbitrate with.
fn classify<A: Clone>(event: &PointerEvent<A>) -> Option<Gesture<A>> {
    Some(match event {
        PointerEvent::Entered(hovered) => Gesture::Entered(hovered.clone()),
        PointerEvent::Exited(hovered) => Gesture::Exited(hovered.clone()),
        PointerEvent::Moved(moved) => Gesture::Moved(*moved),
        PointerEvent::Scrolled(scrolled) => Gesture::Scrolled(*scrolled),
        PointerEvent::Pressed(_)
        | PointerEvent::Released(_)
        | PointerEvent::Dragged(_)
        | PointerEvent::Cancelled(_) => return None,
    })
}

/// Reduces one render-layer pointer event to the public payload.
fn pointer_payload<A: Vocabulary>(
    model: crate::Live2dId,
    kind: PointerEventKind,
) -> Result<PointerEvent<A>, Problem> {
    Ok(match kind {
        PointerEventKind::Pressed {
            pointer,
            area,
            at,
            when,
        } => PointerEvent::Pressed(Pointing {
            model,
            pointer,
            area: optional_key::<A>(area, "hit area")?,
            at,
            when,
        }),
        PointerEventKind::Released {
            pointer,
            area,
            at,
            when,
        } => PointerEvent::Released(Pointing {
            model,
            pointer,
            area: optional_key::<A>(area, "hit area")?,
            at,
            when,
        }),
        PointerEventKind::Cancelled { pointer, when } => PointerEvent::Cancelled(Cancelled {
            model,
            pointer,
            when,
        }),
        PointerEventKind::Moved { at, when } => PointerEvent::Moved(Moving { model, at, when }),
        PointerEventKind::Entered { area, at, when } => PointerEvent::Entered(Hovering {
            model,
            area: key::<A>(area, "hit area")?,
            at,
            when,
        }),
        PointerEventKind::Exited { area, at, when } => PointerEvent::Exited(Hovering {
            model,
            area: key::<A>(area, "hit area")?,
            at,
            when,
        }),
        PointerEventKind::Dragged {
            pointer,
            area,
            origin,
            at,
            when,
        } => PointerEvent::Dragged(Dragging {
            model,
            pointer,
            area: optional_key::<A>(area, "hit area")?,
            origin,
            at,
            when,
        }),
        PointerEventKind::Scrolled { lines, at, when } => PointerEvent::Scrolled(Scrolling {
            lines,
            at,
            when,
            model,
        }),
    })
}

fn key<V: Vocabulary>(id: KeyId, vocabulary: &'static str) -> Result<V, Problem> {
    V::from_id(&id).ok_or(Problem::Vocabulary {
        vocabulary,
        index: id.index,
    })
}

fn optional_key<V: Vocabulary>(
    id: Option<KeyId>,
    vocabulary: &'static str,
) -> Result<Option<V>, Problem> {
    id.map(|id| key::<V>(id, vocabulary)).transpose()
}

impl<Message, I: Interface, M: Mode> Clone for Live2d<'_, Message, I, M> {
    fn clone(&self) -> Self {
        Self {
            model: self.model,
            handlers: self.handlers.clone(),
            gestures: self.gestures,
            mode: PhantomData,
            cursor: self.cursor.clone(),
            look_at: self.look_at,
            capture: self.capture,
            fill: self.fill,
            profile: self.profile,
            width: self.width,
            height: self.height,
        }
    }
}

#[allow(private_bounds)]
impl<'a, Message, I: Interface, M: Plumbing> Live2d<'a, Message, I, M> {
    /// The iced widget that owns layout, the tree slot and the render pass.
    fn delegate(&self) -> shader::Shader<Message, Self> {
        shader::Shader::new(self.clone())
            .width(self.width)
            .height(self.height)
    }

    /// Moves every queued render-layer event into the widget's queue, keeping the
    /// hover state the cursor hook reads and turning pointer events into this mode's
    /// vocabulary on the way.
    fn drain(&self, state: &mut Live2dState<I, M>) {
        loop {
            let Some(raw) = state.events.borrow_mut().try_recv() else {
                return;
            };

            let PipelineEvent::Pointer(kind) = raw else {
                let event = match self.translate(raw) {
                    Ok(event) => event,
                    Err(problem) => Live2dEvent::ModelReportedProblem {
                        model: self.model.id(),
                        problem,
                    },
                };

                state.pending.push_back(event);

                continue;
            };

            let pointer = match pointer_payload::<I::HitArea>(self.model.id(), kind) {
                Ok(pointer) => pointer,
                Err(problem) => {
                    state.pending.push_back(Live2dEvent::ModelReportedProblem {
                        model: self.model.id(),
                        problem,
                    });

                    continue;
                }
            };

            match &pointer {
                PointerEvent::Entered(hovered) => state.hover = Some(hovered.area.clone()),
                PointerEvent::Exited(_) => state.hover = None,
                _ => {}
            }

            M::ingest(&pointer, &mut state.recognizer, &mut state.produced);

            for input in state.produced.drain(..) {
                state.pending.push_back(Live2dEvent::Input(input));
            }
        }
    }

    /// Takes the next queued event, if the host listens for it.
    fn next_message(&self, state: &mut Live2dState<I, M>) -> Option<Message> {
        self.drain(state);

        self.dispatch(state.pending.pop_front()?)
    }

    /// Hands one event to the closure that asked for it, preferring the specific one
    /// over [`Live2d::on_event`].
    fn dispatch(&self, event: Live2dEvent<I, M::Input<I::HitArea>>) -> Option<Message> {
        match event {
            Live2dEvent::Input(input) => {
                let whole = Live2dEvent::Input(input.clone());

                M::dispatch(&self.handlers, &input, whole)
            }
            other => self.handlers.any.as_ref().map(|handle| handle(other)),
        }
    }
}

/// What every mode has to answer: how a pointer event becomes its vocabulary, and
/// which closure a vocabulary member belongs to.
trait Plumbing: Mode + Sized {
    fn ingest<A: Vocabulary + std::fmt::Debug>(
        pointer: &PointerEvent<A>,
        recognizer: &mut Recognizer<A>,
        out: &mut Vec<Self::Input<A>>,
    );

    fn tick<A: Clone + PartialEq + std::fmt::Debug>(
        recognizer: &mut Recognizer<A>,
        now: Instant,
        out: &mut Vec<Self::Input<A>>,
    );

    #[allow(private_interfaces)]
    fn dispatch<Message, I: Interface>(
        handlers: &Handlers<'_, Message, I, Self>,
        input: &Self::Input<I::HitArea>,
        event: Live2dEvent<I, Self::Input<I::HitArea>>,
    ) -> Option<Message>;
}

impl Plumbing for Unset {
    fn ingest<A: Vocabulary + std::fmt::Debug>(
        pointer: &PointerEvent<A>,
        _recognizer: &mut Recognizer<A>,
        out: &mut Vec<PointerEvent<A>>,
    ) {
        out.push(pointer.clone());
    }

    fn tick<A: Clone + PartialEq + std::fmt::Debug>(
        _recognizer: &mut Recognizer<A>,
        _now: Instant,
        _out: &mut Vec<PointerEvent<A>>,
    ) {
    }

    fn dispatch<Message, I: Interface>(
        handlers: &Handlers<'_, Message, I, Self>,
        input: &PointerEvent<I::HitArea>,
        event: Live2dEvent<I, PointerEvent<I::HitArea>>,
    ) -> Option<Message> {
        raw_dispatch(handlers, input, event)
    }
}

impl Plumbing for RawMode {
    fn ingest<A: Vocabulary + std::fmt::Debug>(
        pointer: &PointerEvent<A>,
        _recognizer: &mut Recognizer<A>,
        out: &mut Vec<PointerEvent<A>>,
    ) {
        out.push(pointer.clone());
    }

    fn tick<A: Clone + PartialEq + std::fmt::Debug>(
        _recognizer: &mut Recognizer<A>,
        _now: Instant,
        _out: &mut Vec<PointerEvent<A>>,
    ) {
    }

    fn dispatch<Message, I: Interface>(
        handlers: &Handlers<'_, Message, I, Self>,
        input: &PointerEvent<I::HitArea>,
        event: Live2dEvent<I, PointerEvent<I::HitArea>>,
    ) -> Option<Message> {
        raw_dispatch(handlers, input, event)
    }
}

impl Plumbing for GestureMode {
    fn ingest<A: Vocabulary + std::fmt::Debug>(
        pointer: &PointerEvent<A>,
        recognizer: &mut Recognizer<A>,
        out: &mut Vec<Gesture<A>>,
    ) {
        recognizer.push(pointer, out);

        if let Some(gesture) = classify(pointer) {
            out.push(gesture);
        }
    }

    fn tick<A: Clone + PartialEq + std::fmt::Debug>(
        recognizer: &mut Recognizer<A>,
        now: Instant,
        out: &mut Vec<Gesture<A>>,
    ) {
        recognizer.tick(now, out);
    }

    fn dispatch<Message, I: Interface>(
        handlers: &Handlers<'_, Message, I, Self>,
        input: &Gesture<I::HitArea>,
        event: Live2dEvent<I, Gesture<I::HitArea>>,
    ) -> Option<Message> {
        let specific = match input {
            Gesture::Clicked(tap) => handlers.click.as_ref().map(|handle| handle(tap.clone())),
            Gesture::DoubleClicked(tap) => handlers
                .double_click
                .as_ref()
                .map(|handle| handle(tap.clone())),
            Gesture::LongPressed(tap) => handlers
                .long_press
                .as_ref()
                .map(|handle| handle(tap.clone())),
            Gesture::DragStarted(drag) => handlers
                .drag_start
                .as_ref()
                .map(|handle| handle(drag.clone())),
            Gesture::Dragged(drag) => handlers.drag.as_ref().map(|handle| handle(drag.clone())),
            Gesture::DragEnded(drag) => handlers
                .drag_end
                .as_ref()
                .map(|handle| handle(drag.clone())),
            Gesture::Entered(hovered) => handlers
                .enter
                .as_ref()
                .map(|handle| handle(hovered.clone())),
            Gesture::Exited(hovered) => {
                handlers.exit.as_ref().map(|handle| handle(hovered.clone()))
            }
            Gesture::Scrolled(scrolled) => handlers.scroll.as_ref().map(|handle| handle(*scrolled)),
            Gesture::Moved(_) => {
                return handlers.moved.as_ref().map(|handle| match input {
                    Gesture::Moved(moved) => handle(*moved),
                    _ => unreachable!(),
                });
            }
        };

        specific.or_else(|| handlers.any.as_ref().map(|handle| handle(event)))
    }
}

/// The raw vocabulary's half of [`Plumbing::dispatch`].
fn raw_dispatch<Message, I: Interface, M: Mode>(
    handlers: &Handlers<'_, Message, I, M>,
    input: &PointerEvent<I::HitArea>,
    event: Live2dEvent<I, M::Input<I::HitArea>>,
) -> Option<Message> {
    let specific = match input {
        PointerEvent::Pressed(pointing) => handlers
            .press
            .as_ref()
            .map(|handle| handle(pointing.clone())),
        PointerEvent::Released(pointing) => handlers
            .release
            .as_ref()
            .map(|handle| handle(pointing.clone())),
        PointerEvent::Entered(hovered) => handlers
            .enter
            .as_ref()
            .map(|handle| handle(hovered.clone())),
        PointerEvent::Exited(hovered) => {
            handlers.exit.as_ref().map(|handle| handle(hovered.clone()))
        }
        PointerEvent::Scrolled(scrolled) => {
            handlers.scroll.as_ref().map(|handle| handle(*scrolled))
        }
        PointerEvent::Dragged(dragged) => handlers.drag.as_ref().map(|handle| {
            handle(Drag {
                model: dragged.model,
                pointer: dragged.pointer,
                area: dragged.area.clone(),
                origin: dragged.origin,
                at: dragged.at,
                velocity: [0.0; 2],
            })
        }),
        PointerEvent::Moved(moved) => {
            return handlers.moved.as_ref().map(|handle| handle(*moved));
        }
        PointerEvent::Cancelled(_) => None,
    };

    specific.or_else(|| handlers.any.as_ref().map(|handle| handle(event.clone())))
}

/// The widget's own tree slot: one event channel and one view per instance.
pub struct Live2dState<I: Interface, M: Mode = RawMode> {
    events: RefCell<Channel>,
    pending: VecDeque<Live2dEvent<I, M::Input<I::HitArea>>>,
    recognizer: Recognizer<I::HitArea>,
    produced: Vec<M::Input<I::HitArea>>,
    hover: Option<I::HitArea>,
    pressed: bool,
    view: ViewId,
    presence: Arc<Presence>,
}

/// The events channel, created the first time a widget actually asks for one.
#[derive(Default)]
struct Channel {
    pair: Option<(Sender<PipelineEvent>, Receiver<PipelineEvent>)>,
}

impl Channel {
    fn pair(&mut self) -> &mut (Sender<PipelineEvent>, Receiver<PipelineEvent>) {
        self.pair.get_or_insert_with(crossbeam_channel::unbounded)
    }

    fn sender(&mut self) -> Sender<PipelineEvent> {
        self.pair().0.clone()
    }

    fn try_recv(&mut self) -> Option<PipelineEvent> {
        match &mut self.pair {
            Some((_, receiver)) => receiver.try_recv().ok(),
            None => None,
        }
    }
}

impl<I: Interface, M: Mode> Default for Live2dState<I, M> {
    fn default() -> Self {
        Self {
            events: RefCell::new(Channel::default()),
            pending: VecDeque::new(),
            recognizer: Recognizer::default(),
            produced: Vec::new(),
            hover: None,
            pressed: false,
            view: ViewId::unique(),
            presence: Arc::new(Presence),
        }
    }
}

impl<I: Interface, M: Mode> Live2dState<I, M> {
    fn presence(&self) -> Arc<Presence> {
        Arc::clone(&self.presence)
    }
}

impl<Message, I: Interface, M: Plumbing> shader::Program<Message> for Live2d<'_, Message, I, M> {
    type State = Live2dState<I, M>;
    type Primitive = Live2dPrimitive;

    fn update(
        &self,
        state: &mut Live2dState<I, M>,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<Message>> {
        // Gesture timing uses one clock only: a redraw's timestamp is the instant it
        // was asked for, which is not the instant an event arrived.
        let now = Instant::now();

        let interacting = self.queue_input(state, event, bounds, cursor, now);

        state.recognizer.configure(self.gestures);
        M::tick(&mut state.recognizer, now, &mut state.produced);

        while let Some(input) = state.produced.pop() {
            state.pending.push_front(Live2dEvent::Input(input));
        }

        let message = self.next_message(state);
        let draining = !state.pending.is_empty();

        let mut next_frame = None::<Instant>;

        if let iced::Event::Window(window::Event::RedrawRequested(at)) = event
            && !self.model.is_paused()
        {
            next_frame = Some(*at + REDRAW_LEAD);
        }

        if draining || interacting {
            next_frame =
                Some(next_frame.map_or_else(Instant::now, |frame| frame.min(Instant::now())));
        }

        if let Some(deadline) = state.recognizer.deadline() {
            next_frame = Some(next_frame.map_or(deadline, |frame| frame.min(deadline)));
        }

        let mut action = match (message, next_frame) {
            (Some(message), Some(at)) => shader::Action::publish(message).and_request_redraw_at(at),
            (Some(message), None) => shader::Action::publish(message),
            (None, Some(at)) => shader::Action::request_redraw_at(at),
            (None, None) => return None,
        };

        if self.capture && state.pressed {
            action = action.and_capture();
        }

        Some(action)
    }

    fn draw(
        &self,
        state: &Live2dState<I, M>,
        cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Live2dPrimitive {
        let model = self.model.inner();

        let look_at = match self.look_at {
            Some(source) => Some(
                source(cursor, bounds)
                    .or_else(|| model.target())
                    .unwrap_or([0.0, 0.0]),
            ),
            None => model.target(),
        };

        let inside = if cursor.is_over(bounds) {
            cursor.position()
        } else {
            None
        };

        let events = if self.listens() {
            Some(state.events.borrow_mut().sender())
        } else {
            None
        };

        Live2dPrimitive {
            asset: model.asset(),
            asset_options: model.asset_options(),
            path: model.path_arc(),
            model: model.id(),
            model_options: self.model.options(),
            view: state.view,
            spec: model.spec(),
            fill: self.fill,
            inputs: FrameInputs {
                parameters: model.parameter_writes(),
                parts: model.part_indices(),
                commands: model
                    .drain_commands()
                    .iter()
                    .map(Command::primitive)
                    .collect(),
                problems: model.take_problems(),
                look_at,
                look_at_mode: model.look_at_mode(),
                fade: model.fade(),
                fade_limit: model.fade_limit(),
                physics_enabled: model.physics_enabled(),
                paused: model.paused(),
                cursor: inside,
                position: cursor.position(),
                pointer: model.drain_pointer(),
                profile: self.profile,
            },
            events,
            asset_presence: model.asset_presence(),
            model_presence: model.presence(),
            view_presence: state.presence(),
        }
    }

    fn mouse_interaction(
        &self,
        state: &Live2dState<I, M>,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match &self.cursor {
            Some(hook) => hook(Hover {
                area: state.hover.as_ref(),
                pressed: state.pressed,
            }),
            None => mouse::Interaction::Idle,
        }
    }
}

impl<Message, I: Interface, M: Mode> iced::advanced::widget::Meta for Live2d<'_, Message, I, M> {}

impl<Message, I, M, Theme, Renderer> Widget<Message, Theme, Renderer> for Live2d<'_, Message, I, M>
where
    I: Interface,
    M: Plumbing,
    shader::Shader<Message, Self>: Widget<Message, Theme, Renderer>,
{
    fn tag(&self) -> tree::Tag {
        self.delegate().tag()
    }

    fn state(&self) -> tree::State {
        self.delegate().state()
    }

    fn size(&self) -> iced::Size<Length> {
        self.delegate().size()
    }

    fn layout(&mut self, tree: &mut tree::Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.delegate().layout(tree, renderer, limits);
    }

    fn update(
        &mut self,
        tree: &mut tree::Tree,
        event: &iced::Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.delegate()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &tree::Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.delegate()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &tree::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.delegate()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
}

impl<'a, Message, I: Interface> Live2d<'a, Message, I, Unset> {
    /// Classifies input instead of reporting it raw.
    ///
    /// From here on the raw hooks do not exist and no [`PointerEvent`] is delivered:
    /// one action, one message. See [`Gestures`] for the thresholds.
    pub fn gestures(mut self, gestures: Gestures) -> Live2d<'a, Message, I, GestureMode> {
        self.gestures = Some(gestures);

        self.cast()
    }
}

impl<'a, Message, I: Interface> Live2d<'a, Message, I, GestureMode> {
    /// A press and release on the same hit area, without moving.
    ///
    /// Needs [`Live2d::gestures`].
    pub fn on_click(mut self, on_click: impl Fn(Tap<I::HitArea>) -> Message + 'a) -> Self {
        self.handlers.click = Some(Rc::new(on_click));
        self
    }

    /// Two clicks in quick succession. The second click is not reported twice.
    ///
    /// Needs [`Live2d::gestures`].
    pub fn on_double_click(
        mut self,
        on_double_click: impl Fn(Tap<I::HitArea>) -> Message + 'a,
    ) -> Self {
        self.handlers.double_click = Some(Rc::new(on_double_click));
        self
    }

    /// A press held still for a while, reported while the pointer is still down.
    ///
    /// The release that follows is not reported as a click. Needs
    /// [`Live2d::gestures`].
    pub fn on_long_press(
        mut self,
        on_long_press: impl Fn(Tap<I::HitArea>) -> Message + 'a,
    ) -> Self {
        self.handlers.long_press = Some(Rc::new(on_long_press));
        self
    }

    /// A press travelled far enough to become a drag.
    ///
    /// Needs [`Live2d::gestures`].
    pub fn on_drag_start(
        mut self,
        on_drag_start: impl Fn(Drag<I::HitArea>) -> Message + 'a,
    ) -> Self {
        self.handlers.drag_start = Some(Rc::new(on_drag_start));
        self
    }

    /// The drag ended; the pointer is no longer down.
    ///
    /// Needs [`Live2d::gestures`].
    pub fn on_drag_end(mut self, on_drag_end: impl Fn(Drag<I::HitArea>) -> Message + 'a) -> Self {
        self.handlers.drag_end = Some(Rc::new(on_drag_end));
        self
    }

    /// The pointer moved while held down, once per frame, with its velocity.
    pub fn on_drag(mut self, on_drag: impl Fn(Drag<I::HitArea>) -> Message + 'a) -> Self {
        self.handlers.drag = Some(Rc::new(on_drag));
        self
    }
}

impl<'a, Message, I: Interface> Live2d<'a, Message, I, Unset> {
    /// Reports raw pointer events instead of classifying them, which is what the raw
    /// hooks need.
    pub fn raw(self) -> Live2d<'a, Message, I, RawMode> {
        self.cast()
    }
}

impl<'a, Message, I: Interface> Live2d<'a, Message, I, RawMode> {
    /// A button or finger went down.
    pub fn on_press(mut self, on_press: impl Fn(Pointing<I::HitArea>) -> Message + 'a) -> Self {
        self.handlers.press = Some(Rc::new(on_press));
        self
    }

    /// A button or finger came up, inside the widget or not.
    pub fn on_release(mut self, on_release: impl Fn(Pointing<I::HitArea>) -> Message + 'a) -> Self {
        self.handlers.release = Some(Rc::new(on_release));
        self
    }

    /// The pointer moved while held down, once per frame.
    ///
    /// With [`Live2d::gestures`] the classified [`Gesture::Dragged`] is reported
    /// instead, so this never fires twice for one movement; without it, the raw drag
    /// arrives, which carries no velocity.
    pub fn on_drag(mut self, on_drag: impl Fn(Drag<I::HitArea>) -> Message + 'a) -> Self {
        self.handlers.drag = Some(Rc::new(on_drag));
        self
    }
}
