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
use iced::{Length, Rectangle, mouse, window};
use live2d_render::STEP;

use crate::event::{ModelEvent, Problem};
use crate::gesture::Recognizer;
use crate::interaction::{
    Cancelled, Drag, Dragging, Gesture, Gestures, Hover, Hovering, Moving, Pointer, PointerEvent,
    Pointing, Scrolling, Tap,
};
use crate::interface::{Interface, KeyId, Vocabulary};
use crate::look_at::LookAtSource;
use crate::model::{Command, Live2dModel, PointerInput, Presence};
use crate::pipeline::{
    FrameInputs, Live2dPrimitive, ModelEventKind, PipelineEvent, PointerEventKind, ViewId,
};

/// `1.0` touches the edges; the sample models were framed for this.
const DEFAULT_FILL: f32 = 0.925;

/// Half a step: iced rounds a redraw deadline *up* to the refresh grid, so a full
/// step lands one refresh too late and halves the frame rate.
const REDRAW_LEAD: Duration = Duration::from_micros(STEP.as_micros() as u64 / 2);

/// Pixels a scroll notch carries, when the platform reports pixels.
const PIXELS_PER_LINE: f32 = 50.0;

/// How many events may wait for the host before the oldest are dropped.
const MAX_PENDING: usize = 64;

/// Starts a model widget: `live2d(&self.hiyori).on_model_event(Message::Live2d)`.
///
/// The widget borrows the model, so the model must outlive the UI tree. Two
/// widgets may draw one model; each frames it and reports pointer input in its own
/// space, but only one of them owns the pointer at a time, and model events reach
/// both.
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

/// A chosen vocabulary, i.e. a widget that can report interaction input at all.
/// [`Unset`] is not one: it has to pick a vocabulary first.
pub trait InputMode: Mode + sealed::InputMode {}

/// Nothing chosen yet.
pub struct Unset;

/// Raw pointer facts only, each with the instant it happened. A double click is two
/// of those, and telling them apart is the host's business.
pub struct RawMode;

/// Classified input only: clicks, double clicks, long presses, drags, hover, scroll.
/// No raw pointer event is delivered.
pub struct GestureMode;

/// The widget binds no interaction handler.
pub struct NoInputHandler;

/// The widget binds specific interaction hooks.
pub struct SpecificInput;

/// The widget binds the catch-all [`Live2d::on_event`], which excludes every
/// specific interaction hook.
pub struct CatchAllInput;

mod sealed {
    pub trait Mode {}
    pub trait InputMode {}
    pub trait HandlerState {}

    impl Mode for super::Unset {}
    impl Mode for super::RawMode {}
    impl Mode for super::GestureMode {}

    impl InputMode for super::RawMode {}
    impl InputMode for super::GestureMode {}

    impl HandlerState for super::NoInputHandler {}
    impl HandlerState for super::SpecificInput {}
    impl HandlerState for super::CatchAllInput {}
}

/// The interaction handlers a widget has bound. Chosen by the binding methods, not
/// by the host.
pub trait HandlerState: sealed::HandlerState + 'static {}

impl<T: sealed::HandlerState + 'static> HandlerState for T {}

/// A handler state that can still take a specific interaction hook.
pub trait AcceptsSpecific: HandlerState {}

impl AcceptsSpecific for NoInputHandler {}
impl AcceptsSpecific for SpecificInput {}

impl Mode for Unset {
    type Input<A: Clone + std::fmt::Debug + 'static> = PointerEvent<A>;
}

impl Mode for RawMode {
    type Input<A: Clone + std::fmt::Debug + 'static> = PointerEvent<A>;
}

impl Mode for GestureMode {
    type Input<A: Clone + std::fmt::Debug + 'static> = Gesture<A>;
}

impl InputMode for RawMode {}
impl InputMode for GestureMode {}

/// A Live2D model drawn as a widget. wgpu only; the tiny-skia backend cannot draw
/// iced's `shader` widget. Box it with `column![…]` or `container`.
///
/// The widget drives its own frames. A host must **not** also subscribe to
/// [`window::frames`]: `RedrawRequest` holds one request and the last writer wins.
///
/// Input reaches the host through `on_event` or the specific `on_*` hooks, and the
/// model's own events through `on_model_event`. Nothing is decided here: the
/// component reports what happened and the host decides what it means.
pub struct Live2d<'a, Message, I: Interface, M: Mode = Unset, H: HandlerState = NoInputHandler> {
    model: &'a Live2dModel<I>,
    handlers: Handlers<'a, Message, I, M>,
    gestures: Option<Gestures>,
    mode: PhantomData<M>,
    handler_state: PhantomData<H>,
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

/// The cursor hook: a pure function of hover, so it costs no message.
type Cursor<'a, A> = Rc<dyn Fn(Hover<'_, A>) -> mouse::Interaction + 'a>;

/// One closure per input the host asked for. A missing closure means "not
/// interested", including for the frame request that input needs.
///
/// `any` and the specific hooks never coexist: the handler state makes binding
/// both impossible.
struct Handlers<'a, Message, I: Interface, M: Mode> {
    model: Option<Handler<'a, ModelEvent<I>, Message>>,
    any: Option<Handler<'a, M::Input<I::HitArea>, Message>>,
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
            model: None,
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
            model: self.model.clone(),
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

impl<'a, Message, I: Interface, M: Mode> Handlers<'a, Message, I, M> {
    /// Whether any input at all is wanted, whether interaction or model events.
    fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.any.is_none()
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

    /// The same hooks under another vocabulary. A vocabulary is chosen before any
    /// interaction handler is bound, so there is no catch-all to carry over.
    fn into_mode<M2: Mode>(self) -> Handlers<'a, Message, I, M2> {
        Handlers {
            model: self.model,
            any: None,
            press: self.press,
            release: self.release,
            click: self.click,
            double_click: self.double_click,
            long_press: self.long_press,
            drag_start: self.drag_start,
            drag: self.drag,
            drag_end: self.drag_end,
            scroll: self.scroll,
            enter: self.enter,
            exit: self.exit,
            moved: self.moved,
        }
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
            handler_state: PhantomData,
            cursor: None,
            look_at: None,
            capture: false,
            fill: DEFAULT_FILL,
            profile: false,
            width: Length::Fill,
            height: Length::Fill,
        }
    }
}

impl<'a, Message, I: Interface, M: Mode, H: HandlerState> Live2d<'a, Message, I, M, H> {
    /// The same widget under another handler state.
    fn into_handler_state<H2: HandlerState>(self) -> Live2d<'a, Message, I, M, H2> {
        Live2d {
            model: self.model,
            handlers: self.handlers,
            gestures: self.gestures,
            mode: PhantomData,
            handler_state: PhantomData,
            cursor: self.cursor,
            look_at: self.look_at,
            capture: self.capture,
            fill: self.fill,
            profile: self.profile,
            width: self.width,
            height: self.height,
        }
    }

    /// The same widget under another vocabulary.
    fn into_mode<M2: Mode>(self) -> Live2d<'a, Message, I, M2, H> {
        Live2d {
            model: self.model,
            handlers: self.handlers.into_mode(),
            gestures: self.gestures,
            mode: PhantomData,
            handler_state: PhantomData,
            cursor: self.cursor,
            look_at: self.look_at,
            capture: self.capture,
            fill: self.fill,
            profile: self.profile,
            width: self.width,
            height: self.height,
        }
    }

    /// Sends every [`ModelEvent`] to the host as `Message`.
    ///
    /// Without it model events are dropped, load failures included. At most one per
    /// frame, one frame late; a burst is queued, not lost.
    ///
    /// Model events are a channel of their own: they can be combined with
    /// [`Live2d::on_event`] or with any specific hook, because the two categories
    /// never overlap.
    pub fn on_model_event(
        mut self,
        on_model_event: impl Fn(ModelEvent<I>) -> Message + 'a,
    ) -> Self {
        self.handlers.model = Some(Rc::new(on_model_event));
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

    /// Whether the render layer should report anything to this widget at all.
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
    /// is frozen: it still has a pose to hit-test.
    fn queue_input(
        &self,
        state: &mut Live2dState<I, M>,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        when: Instant,
    ) -> bool {
        let model = self.model.inner();
        let view = state.presence();

        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(button)) if cursor.is_over(bounds) => {
                let pointer = Pointer::Mouse(*button);

                if !model.claim_focus(&view) {
                    return false;
                }

                state.press(pointer);

                if let Some(at) = cursor.position() {
                    state.push_pointer(PointerInput::Pressed { pointer, at, when });
                }
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(button)) => {
                let pointer = Pointer::Mouse(*button);

                if !state.release(pointer) {
                    return false;
                }

                if let Some(at) = cursor.position() {
                    state.push_pointer(PointerInput::Released { pointer, at, when });
                }

                if state.pressed.borrow().is_empty() {
                    model.clear_focus(&view);
                }
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                if !model.focus_allows(&view) {
                    return false;
                }

                let lines = match delta {
                    mouse::ScrollDelta::Lines { x, y } => [*x, *y],
                    mouse::ScrollDelta::Pixels { x, y } => {
                        [*x / PIXELS_PER_LINE, *y / PIXELS_PER_LINE]
                    }
                };

                if let Some(at) = cursor.position() {
                    state.push_pointer(PointerInput::Scrolled { lines, at, when });
                }
            }
            iced::Event::Window(window::Event::Unfocused) => {
                if !model.focus_allows(&view) {
                    return false;
                }

                let held = !state.pressed.borrow().is_empty();
                state.pressed.borrow_mut().clear();
                model.clear_focus(&view);

                if !held {
                    return false;
                }

                state.push_pointer(PointerInput::CancelAll);
            }
            _ => return false,
        }

        true
    }

    /// Turns a model event reported by the render layer into the host's.
    fn translate_model(&self, kind: ModelEventKind) -> Result<ModelEvent<I>, Problem> {
        let model = self.model.id();

        Ok(match kind {
            ModelEventKind::Loaded { info } => ModelEvent::Loaded { model, info },
            ModelEventKind::LoadFailed { failure } => ModelEvent::FailedToLoad { model, failure },
            ModelEventKind::Problem(problem) => ModelEvent::Problem { model, problem },
            ModelEventKind::MotionStarted {
                group,
                clip,
                duration,
                looping,
            } => ModelEvent::MotionStarted {
                model,
                group: key::<I::Motion>(group, "motion")?,
                clip,
                duration,
                looping,
            },
            ModelEventKind::MotionEnded { group, clip, ended } => ModelEvent::MotionEnded {
                model,
                group: key::<I::Motion>(group, "motion")?,
                clip,
                ended,
            },
        })
    }
}

impl<'a, Message, I: Interface, M: InputMode> Live2d<'a, Message, I, M, NoInputHandler> {
    /// Sends every interaction event of this vocabulary to the host as `Message`.
    ///
    /// This is the catch-all: it receives every pointer event the widget reports,
    /// including movement, and is exclusive with the specific hooks. Bind model
    /// events with [`Live2d::on_model_event`].
    pub fn on_event(
        mut self,
        on_event: impl Fn(M::Input<I::HitArea>) -> Message + 'a,
    ) -> Live2d<'a, Message, I, M, CatchAllInput> {
        self.handlers.any = Some(Rc::new(on_event));
        self.into_handler_state()
    }
}

impl<'a, Message, I: Interface, M: InputMode, H: AcceptsSpecific> Live2d<'a, Message, I, M, H> {
    /// The wheel turned over the widget.
    pub fn on_scroll(
        mut self,
        on_scroll: impl Fn(Scrolling) -> Message + 'a,
    ) -> Live2d<'a, Message, I, M, SpecificInput> {
        self.handlers.scroll = Some(Rc::new(on_scroll));
        self.into_handler_state()
    }

    /// The pointer came onto a hit area.
    pub fn on_enter(
        mut self,
        on_enter: impl Fn(Hovering<I::HitArea>) -> Message + 'a,
    ) -> Live2d<'a, Message, I, M, SpecificInput> {
        self.handlers.enter = Some(Rc::new(on_enter));
        self.into_handler_state()
    }

    /// The pointer left a hit area.
    pub fn on_exit(
        mut self,
        on_exit: impl Fn(Hovering<I::HitArea>) -> Message + 'a,
    ) -> Live2d<'a, Message, I, M, SpecificInput> {
        self.handlers.exit = Some(Rc::new(on_exit));
        self.into_handler_state()
    }

    /// The pointer moved over the widget, once per frame.
    ///
    /// Only reported when this is set: it is the one continuous input, and a host
    /// that does not need it should not pay a message per frame.
    pub fn on_move(
        mut self,
        on_move: impl Fn(Moving) -> Message + 'a,
    ) -> Live2d<'a, Message, I, M, SpecificInput> {
        self.handlers.moved = Some(Rc::new(on_move));
        self.into_handler_state()
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
    V::from_id(&id).map_err(|error| Problem::Vocabulary {
        vocabulary,
        index: error.index(),
    })
}

fn optional_key<V: Vocabulary>(
    id: Option<KeyId>,
    vocabulary: &'static str,
) -> Result<Option<V>, Problem> {
    id.map(|id| key::<V>(id, vocabulary)).transpose()
}

impl<Message, I: Interface, M: Mode, H: HandlerState> Clone for Live2d<'_, Message, I, M, H> {
    fn clone(&self) -> Self {
        Self {
            model: self.model,
            handlers: self.handlers.clone(),
            gestures: self.gestures,
            mode: PhantomData,
            handler_state: PhantomData,
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
impl<'a, Message, I: Interface, M: Plumbing, H: HandlerState> Live2d<'a, Message, I, M, H> {
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

            match raw {
                PipelineEvent::Model(kind) => {
                    let event = match self.translate_model(kind) {
                        Ok(event) => event,
                        Err(problem) => ModelEvent::Problem {
                            model: self.model.id(),
                            problem,
                        },
                    };

                    state.push(PendingEvent::Model(event));
                }
                PipelineEvent::Pointer(kind) => {
                    let pointer = match pointer_payload::<I::HitArea>(self.model.id(), kind) {
                        Ok(pointer) => pointer,
                        Err(problem) => {
                            state.push(PendingEvent::Model(ModelEvent::Problem {
                                model: self.model.id(),
                                problem,
                            }));

                            continue;
                        }
                    };

                    match &pointer {
                        PointerEvent::Entered(hovered) => state.hover = Some(hovered.area.clone()),
                        PointerEvent::Exited(_) => state.hover = None,
                        _ => {}
                    }

                    M::ingest(&pointer, &mut state.recognizer, &mut state.produced);

                    for input in std::mem::take(&mut state.produced) {
                        state.push_input(input);
                    }
                }
            }
        }
    }

    /// Takes the next queued event, if the host listens for it.
    fn next_message(&self, state: &mut Live2dState<I, M>) -> Option<Message> {
        self.drain(state);
        self.report_overflow(state);

        match state.pending.pop_front()? {
            PendingEvent::Model(event) => self.handlers.model.as_ref().map(|handle| handle(event)),
            PendingEvent::Input(input) => M::dispatch(&self.handlers, &input),
        }
    }

    /// Turns a run of dropped events into one model problem.
    ///
    /// Reporting waits for room: a problem that itself overflowed the queue would
    /// starve the events ahead of it and report again on every frame.
    fn report_overflow(&self, state: &mut Live2dState<I, M>) {
        if state.dropped == 0 || state.pending.len() >= MAX_PENDING {
            return;
        }

        let dropped = std::mem::take(&mut state.dropped);

        state.push(PendingEvent::Model(ModelEvent::Problem {
            model: self.model.id(),
            problem: Problem::EventQueueOverflow { dropped },
        }));
    }
}

/// One event waiting for the host: the two categories share a queue so that their
/// order is the order they happened in.
enum PendingEvent<I: Interface, In> {
    Model(ModelEvent<I>),
    Input(In),
}

/// What a repeated input coalesces onto: only the latest position matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoalesceKey {
    Moved,
    Dragged(Pointer),
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

    /// The events this one makes redundant, if any.
    fn coalesce_key<A: Clone + std::fmt::Debug + 'static>(
        input: &Self::Input<A>,
    ) -> Option<CoalesceKey>;

    #[allow(private_interfaces)]
    fn dispatch<Message, I: Interface>(
        handlers: &Handlers<'_, Message, I, Self>,
        input: &Self::Input<I::HitArea>,
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

    fn coalesce_key<A: Clone + std::fmt::Debug + 'static>(
        _input: &PointerEvent<A>,
    ) -> Option<CoalesceKey> {
        None
    }

    fn dispatch<Message, I: Interface>(
        handlers: &Handlers<'_, Message, I, Self>,
        input: &PointerEvent<I::HitArea>,
    ) -> Option<Message> {
        raw_specific(handlers, input)
            .or_else(|| handlers.any.as_ref().map(|handle| handle(input.clone())))
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

    fn coalesce_key<A: Clone + std::fmt::Debug + 'static>(
        input: &PointerEvent<A>,
    ) -> Option<CoalesceKey> {
        match input {
            PointerEvent::Moved(_) => Some(CoalesceKey::Moved),
            PointerEvent::Dragged(dragged) => Some(CoalesceKey::Dragged(dragged.pointer)),
            _ => None,
        }
    }

    fn dispatch<Message, I: Interface>(
        handlers: &Handlers<'_, Message, I, Self>,
        input: &PointerEvent<I::HitArea>,
    ) -> Option<Message> {
        raw_specific(handlers, input)
            .or_else(|| handlers.any.as_ref().map(|handle| handle(input.clone())))
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

    fn coalesce_key<A: Clone + std::fmt::Debug + 'static>(
        input: &Gesture<A>,
    ) -> Option<CoalesceKey> {
        match input {
            Gesture::Moved(_) => Some(CoalesceKey::Moved),
            Gesture::Dragged(dragged) => Some(CoalesceKey::Dragged(dragged.pointer)),
            _ => None,
        }
    }

    fn dispatch<Message, I: Interface>(
        handlers: &Handlers<'_, Message, I, Self>,
        input: &Gesture<I::HitArea>,
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
            Gesture::Moved(moved) => handlers.moved.as_ref().map(|handle| handle(*moved)),
        };

        specific.or_else(|| handlers.any.as_ref().map(|handle| handle(input.clone())))
    }
}

/// The specific raw hooks, without the catch-all fallback.
fn raw_specific<Message, I: Interface, M: Mode>(
    handlers: &Handlers<'_, Message, I, M>,
    input: &PointerEvent<I::HitArea>,
) -> Option<Message> {
    match input {
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
        PointerEvent::Moved(moved) => handlers.moved.as_ref().map(|handle| handle(*moved)),
        PointerEvent::Cancelled(_) => None,
    }
}

/// The widget's own tree slot: one event channel and one view per instance.
pub struct Live2dState<I: Interface, M: Mode = RawMode> {
    events: RefCell<Channel>,
    pending: VecDeque<PendingEvent<I, M::Input<I::HitArea>>>,
    recognizer: Recognizer<I::HitArea>,
    produced: Vec<M::Input<I::HitArea>>,
    hover: Option<I::HitArea>,
    /// The buttons this view is holding down. Only the focus view has any.
    pressed: RefCell<Vec<Pointer>>,
    /// Pointer input waiting for the next `draw`, which is where it reaches the
    /// render layer. Per view: this is the queue two widgets on one model do not
    /// share.
    pointer: RefCell<Vec<PointerInput>>,
    /// Events dropped to keep the queue bounded, not yet reported.
    dropped: usize,
    view: ViewId,
    presence: Arc<Presence>,
}

impl<I: Interface, M: Mode> Live2dState<I, M> {
    fn presence(&self) -> Arc<Presence> {
        Arc::clone(&self.presence)
    }

    /// Records that this view now holds `pointer` down.
    fn press(&mut self, pointer: Pointer) {
        let mut pressed = self.pressed.borrow_mut();

        if !pressed.contains(&pointer) {
            pressed.push(pointer);
        }
    }

    /// Forgets `pointer`, reporting whether this view was holding it.
    fn release(&mut self, pointer: Pointer) -> bool {
        let mut pressed = self.pressed.borrow_mut();

        match pressed.iter().position(|held| *held == pointer) {
            Some(index) => {
                pressed.remove(index);
                true
            }
            None => false,
        }
    }

    /// Queues one pointer input for the next `draw`.
    fn push_pointer(&mut self, input: PointerInput) {
        self.pointer.borrow_mut().push(input);
    }

    /// Queues one event, dropping the oldest once the queue is full.
    fn push(&mut self, event: PendingEvent<I, M::Input<I::HitArea>>) {
        if self.pending.len() >= MAX_PENDING {
            self.pending.pop_front();
            self.dropped += 1;
        }

        self.pending.push_back(event);
    }
}

#[allow(private_bounds)]
impl<I: Interface, M: Plumbing> Live2dState<I, M> {
    /// Queues interaction input, keeping only the latest of a repeated stream.
    fn push_input(&mut self, input: M::Input<I::HitArea>) {
        if let Some(key) = M::coalesce_key(&input) {
            self.pending.retain(|event| match event {
                PendingEvent::Input(existing) => M::coalesce_key(existing) != Some(key),
                PendingEvent::Model(_) => true,
            });
        }

        self.push(PendingEvent::Input(input));
    }
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
            pressed: RefCell::new(Vec::new()),
            pointer: RefCell::new(Vec::new()),
            dropped: 0,
            view: ViewId::unique(),
            presence: Arc::new(Presence),
        }
    }
}

impl<Message, I: Interface, M: Plumbing, H: HandlerState> shader::Program<Message>
    for Live2d<'_, Message, I, M, H>
{
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

        for input in std::mem::take(&mut state.produced) {
            state.push_input(input);
        }

        let message = self.next_message(state);
        let draining = !state.pending.is_empty();

        let mut next_frame = None::<Instant>;

        if let iced::Event::Window(window::Event::RedrawRequested(at)) = event
            && !self.model.is_frozen()
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

        if self.capture && !state.pressed.borrow().is_empty() {
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

        let observing = model.focus_allows(&state.presence);

        let inside = if cursor.is_over(bounds) && observing {
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
                frozen: model.frozen(),
                cursor: inside,
                position: observing.then(|| cursor.position()).flatten(),
                pointer: state.pointer.borrow_mut().drain(..).collect(),
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
                pressed: !state.pressed.borrow().is_empty(),
            }),
            None => mouse::Interaction::Idle,
        }
    }
}

impl<Message, I: Interface, M: Mode, H: HandlerState> iced::advanced::widget::Meta
    for Live2d<'_, Message, I, M, H>
{
}

impl<Message, I, M, H, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Live2d<'_, Message, I, M, H>
where
    I: Interface,
    M: Plumbing,
    H: HandlerState,
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

impl<'a, Message, I: Interface> Live2d<'a, Message, I, Unset, NoInputHandler> {
    /// Classifies input instead of reporting it raw.
    ///
    /// From here on the raw hooks do not exist and no [`PointerEvent`] is delivered:
    /// one action, one message. See [`Gestures`] for the thresholds.
    pub fn gestures(self, gestures: Gestures) -> Live2d<'a, Message, I, GestureMode> {
        let mut widget: Live2d<'_, Message, I, GestureMode, NoInputHandler> = self.into_mode();
        widget.gestures = Some(gestures);
        widget
    }

    /// Reports raw pointer events instead of classifying them, which is what the raw
    /// hooks need.
    pub fn raw(self) -> Live2d<'a, Message, I, RawMode> {
        self.into_mode()
    }
}

impl<'a, Message, I: Interface, H: AcceptsSpecific> Live2d<'a, Message, I, GestureMode, H> {
    /// A press and release on the same hit area, without moving.
    ///
    /// Needs [`Live2d::gestures`].
    pub fn on_click(
        mut self,
        on_click: impl Fn(Tap<I::HitArea>) -> Message + 'a,
    ) -> Live2d<'a, Message, I, GestureMode, SpecificInput> {
        self.handlers.click = Some(Rc::new(on_click));
        self.into_handler_state()
    }

    /// Two clicks in quick succession. The second click is not reported twice.
    ///
    /// Needs [`Live2d::gestures`].
    pub fn on_double_click(
        mut self,
        on_double_click: impl Fn(Tap<I::HitArea>) -> Message + 'a,
    ) -> Live2d<'a, Message, I, GestureMode, SpecificInput> {
        self.handlers.double_click = Some(Rc::new(on_double_click));
        self.into_handler_state()
    }

    /// A press held still for a while, reported while the pointer is still down.
    ///
    /// The release that follows is not reported as a click. Needs
    /// [`Live2d::gestures`].
    pub fn on_long_press(
        mut self,
        on_long_press: impl Fn(Tap<I::HitArea>) -> Message + 'a,
    ) -> Live2d<'a, Message, I, GestureMode, SpecificInput> {
        self.handlers.long_press = Some(Rc::new(on_long_press));
        self.into_handler_state()
    }

    /// A press travelled far enough to become a drag.
    ///
    /// Needs [`Live2d::gestures`].
    pub fn on_drag_start(
        mut self,
        on_drag_start: impl Fn(Drag<I::HitArea>) -> Message + 'a,
    ) -> Live2d<'a, Message, I, GestureMode, SpecificInput> {
        self.handlers.drag_start = Some(Rc::new(on_drag_start));
        self.into_handler_state()
    }

    /// The drag ended; the pointer is no longer down.
    ///
    /// Needs [`Live2d::gestures`].
    pub fn on_drag_end(
        mut self,
        on_drag_end: impl Fn(Drag<I::HitArea>) -> Message + 'a,
    ) -> Live2d<'a, Message, I, GestureMode, SpecificInput> {
        self.handlers.drag_end = Some(Rc::new(on_drag_end));
        self.into_handler_state()
    }

    /// The pointer moved while held down, once per frame, with its velocity.
    pub fn on_drag(
        mut self,
        on_drag: impl Fn(Drag<I::HitArea>) -> Message + 'a,
    ) -> Live2d<'a, Message, I, GestureMode, SpecificInput> {
        self.handlers.drag = Some(Rc::new(on_drag));
        self.into_handler_state()
    }
}

impl<'a, Message, I: Interface, H: AcceptsSpecific> Live2d<'a, Message, I, RawMode, H> {
    /// A button went down.
    pub fn on_press(
        mut self,
        on_press: impl Fn(Pointing<I::HitArea>) -> Message + 'a,
    ) -> Live2d<'a, Message, I, RawMode, SpecificInput> {
        self.handlers.press = Some(Rc::new(on_press));
        self.into_handler_state()
    }

    /// A button came up, inside the widget or not.
    pub fn on_release(
        mut self,
        on_release: impl Fn(Pointing<I::HitArea>) -> Message + 'a,
    ) -> Live2d<'a, Message, I, RawMode, SpecificInput> {
        self.handlers.release = Some(Rc::new(on_release));
        self.into_handler_state()
    }

    /// The pointer moved while held down, once per frame.
    ///
    /// With [`Live2d::gestures`] the classified [`Gesture::Dragged`] is reported
    /// instead, so this never fires twice for one movement; without it, the raw drag
    /// arrives, which carries no velocity.
    pub fn on_drag(
        mut self,
        on_drag: impl Fn(Drag<I::HitArea>) -> Message + 'a,
    ) -> Live2d<'a, Message, I, RawMode, SpecificInput> {
        self.handlers.drag = Some(Rc::new(on_drag));
        self.into_handler_state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interaction::Sample;
    use crate::model::Live2dAsset;
    use crate::raw::Raw;
    use live2d_render::{AssetOptions, ModelOptions};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Kind {
        Model,
        Input,
    }

    fn model() -> Live2dModel<Raw> {
        let asset = Live2dAsset::new("model.model3.json", AssetOptions::default());

        Live2dModel::<Raw>::new(&asset, ModelOptions::default())
    }

    fn state() -> Live2dState<Raw, RawMode> {
        Live2dState::default()
    }

    fn send(state: &mut Live2dState<Raw, RawMode>, event: PipelineEvent) {
        state
            .events
            .borrow_mut()
            .sender()
            .send(event)
            .expect("the channel is open");
    }

    fn sample() -> Sample {
        Sample {
            pixels: iced::Point::ORIGIN,
            clip: [0.0, 0.0],
            model: [0.0, 0.0],
        }
    }

    fn moved() -> PipelineEvent {
        PipelineEvent::Pointer(PointerEventKind::Moved {
            at: sample(),
            when: Instant::now(),
        })
    }

    /// A vocabulary that disagrees with the render layer about its own keys.
    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    struct Broken;

    impl Vocabulary for Broken {
        const SPEC: crate::VocabularySpec = crate::VocabularySpec::Static(&["A"]);

        fn name(&self) -> &str {
            "A"
        }

        fn index(&self) -> usize {
            0
        }

        fn from_id(id: &KeyId) -> Result<Self, crate::VocabularyError> {
            Err(crate::VocabularyError::new(id.index))
        }
    }

    #[test]
    fn a_key_the_vocabulary_cannot_rebuild_is_reported() {
        let id = KeyId {
            asset: live2d_render::AssetId::unique(),
            index: 3,
            name: Arc::from("A"),
        };

        assert!(matches!(
            key::<Broken>(id, "motion"),
            Err(Problem::Vocabulary { index: 3, .. })
        ));
    }

    fn bounds() -> Rectangle {
        Rectangle::new(iced::Point::ORIGIN, iced::Size::new(100.0, 100.0))
    }

    fn over() -> mouse::Cursor {
        mouse::Cursor::Available(iced::Point::new(10.0, 10.0))
    }

    fn press(button: mouse::Button) -> iced::Event {
        iced::Event::Mouse(mouse::Event::ButtonPressed(button))
    }

    fn release(button: mouse::Button) -> iced::Event {
        iced::Event::Mouse(mouse::Event::ButtonReleased(button))
    }

    fn unfocused() -> iced::Event {
        iced::Event::Window(window::Event::Unfocused)
    }

    fn plain<'a>(model: &'a Live2dModel<Raw>) -> Live2d<'a, (), Raw, RawMode> {
        live2d(model).raw()
    }

    fn draw(
        widget: &Live2d<'_, (), Raw, RawMode>,
        state: &Live2dState<Raw, RawMode>,
    ) -> Live2dPrimitive {
        shader::Program::<()>::draw(widget, state, over(), bounds())
    }

    #[test]
    fn only_the_focus_view_queues_a_press() {
        let model = model();
        let widget = plain(&model);
        let (mut a, mut b) = (state(), state());

        assert!(widget.queue_input(
            &mut a,
            &press(mouse::Button::Left),
            bounds(),
            over(),
            Instant::now()
        ));
        assert!(!widget.queue_input(
            &mut b,
            &press(mouse::Button::Left),
            bounds(),
            over(),
            Instant::now()
        ));

        assert_eq!(a.pointer.borrow().len(), 1);
        assert!(b.pointer.borrow().is_empty());
    }

    #[test]
    fn releasing_the_last_button_frees_the_pointer() {
        let model = model();
        let widget = plain(&model);
        let (mut a, mut b) = (state(), state());

        widget.queue_input(
            &mut a,
            &press(mouse::Button::Left),
            bounds(),
            over(),
            Instant::now(),
        );
        widget.queue_input(
            &mut b,
            &press(mouse::Button::Left),
            bounds(),
            over(),
            Instant::now(),
        );
        widget.queue_input(
            &mut a,
            &release(mouse::Button::Left),
            bounds(),
            over(),
            Instant::now(),
        );

        assert!(widget.queue_input(
            &mut b,
            &press(mouse::Button::Left),
            bounds(),
            over(),
            Instant::now()
        ));

        assert_eq!(b.pointer.borrow().len(), 1);
    }

    #[test]
    fn releasing_one_button_keeps_the_other() {
        let model = model();
        let widget = plain(&model);
        let (mut a, mut b) = (state(), state());

        widget.queue_input(
            &mut a,
            &press(mouse::Button::Left),
            bounds(),
            over(),
            Instant::now(),
        );
        widget.queue_input(
            &mut a,
            &press(mouse::Button::Right),
            bounds(),
            over(),
            Instant::now(),
        );
        widget.queue_input(
            &mut a,
            &release(mouse::Button::Right),
            bounds(),
            over(),
            Instant::now(),
        );

        assert!(!widget.queue_input(
            &mut b,
            &press(mouse::Button::Left),
            bounds(),
            over(),
            Instant::now()
        ));

        widget.queue_input(
            &mut a,
            &release(mouse::Button::Left),
            bounds(),
            over(),
            Instant::now(),
        );

        assert!(widget.queue_input(
            &mut b,
            &press(mouse::Button::Left),
            bounds(),
            over(),
            Instant::now()
        ));
    }

    #[test]
    fn a_view_without_focus_reports_no_movement_and_ignores_the_wheel() {
        let model = model();
        let widget = plain(&model);
        let (mut a, mut b) = (state(), state());

        widget.queue_input(
            &mut a,
            &press(mouse::Button::Left),
            bounds(),
            over(),
            Instant::now(),
        );

        let wheel = iced::Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
        });

        assert!(!widget.queue_input(&mut b, &wheel, bounds(), over(), Instant::now()));
        assert!(draw(&widget, &b).inputs.cursor.is_none());
        assert!(draw(&widget, &a).inputs.cursor.is_some());
    }

    #[test]
    fn losing_focus_cancels_the_held_press() {
        let model = model();
        let widget = plain(&model);
        let mut a = state();

        widget.queue_input(
            &mut a,
            &press(mouse::Button::Left),
            bounds(),
            over(),
            Instant::now(),
        );

        let cancelling = unfocused();

        assert!(widget.queue_input(&mut a, &cancelling, bounds(), over(), Instant::now()));

        assert!(a.pressed.borrow().is_empty());
        assert!(matches!(
            a.pointer.borrow().last(),
            Some(PointerInput::CancelAll)
        ));
    }

    #[test]
    fn on_event_receives_pointer_movement() {
        let model = model();
        let widget = live2d(&model).raw().on_event(|_| Kind::Input);
        let mut state = state();

        send(&mut state, moved());

        assert_eq!(widget.next_message(&mut state), Some(Kind::Input));
    }

    #[test]
    fn model_events_and_input_reach_their_own_handlers() {
        let model = model();
        let widget = live2d(&model)
            .raw()
            .on_event(|_| Kind::Input)
            .on_model_event(|_| Kind::Model);
        let mut state = state();

        send(
            &mut state,
            PipelineEvent::Model(ModelEventKind::Problem(Problem::CommandQueueOverflow {
                dropped: 1,
            })),
        );
        send(&mut state, moved());

        assert_eq!(widget.next_message(&mut state), Some(Kind::Model));
        assert_eq!(widget.next_message(&mut state), Some(Kind::Input));
    }

    #[test]
    fn repeated_movement_coalesces_to_the_latest() {
        let model = model();
        let widget = live2d(&model).raw().on_event(|_| Kind::Input);
        let mut state = state();

        send(&mut state, moved());
        send(&mut state, moved());

        assert_eq!(widget.next_message(&mut state), Some(Kind::Input));
        assert_eq!(widget.next_message(&mut state), None);
    }

    #[test]
    fn the_queue_drops_the_oldest_past_the_cap_and_reports_it_once() {
        let model = model();
        let widget = live2d(&model)
            .raw()
            .on_event(|_| Kind::Input)
            .on_model_event(|_| Kind::Model);
        let mut state = state();

        for _ in 0..MAX_PENDING + 6 {
            send(
                &mut state,
                PipelineEvent::Pointer(PointerEventKind::Scrolled {
                    lines: [0.0, 1.0],
                    at: sample(),
                    when: Instant::now(),
                }),
            );
        }

        assert_eq!(widget.next_message(&mut state), Some(Kind::Input));
        assert_eq!(state.dropped, 6);
        assert_eq!(state.pending.len(), MAX_PENDING - 1);

        let mut messages = Vec::new();

        while let Some(message) = widget.next_message(&mut state) {
            messages.push(message);
        }

        assert_eq!(
            messages.iter().filter(|kind| **kind == Kind::Model).count(),
            1
        );
        assert_eq!(messages.len(), MAX_PENDING);
    }
}
