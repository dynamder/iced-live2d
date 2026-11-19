//! The widget.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use iced::advanced::layout;
use iced::advanced::widget::tree;
use iced::advanced::{Layout, Widget, renderer};
use iced::widget::shader;
use iced::{Length, Rectangle, mouse, window};
use live2d_render::STEP;

use crate::event::{Live2dEvent, PointerEvent, Problem};
use crate::interface::{Interface, KeyId, Vocabulary};
use crate::look_at::LookAtSource;
use crate::model::{Command, Idle, Live2dModel, PointerInput, Presence};
use crate::pipeline::{
    FrameInputs, IdleInput, Live2dPrimitive, PipelineEvent, PointerEventKind, ViewId,
};

/// How much of the widget the model's bounds fill by default.
///
/// The sample models were framed at 1.85 out of the 2.0 clip-space range, so this
/// is the framing they were verified against; `.fill(1.0)` touches the edges.
const DEFAULT_FILL: f32 = 0.925;

/// How far ahead of the current frame to ask for the next one.
///
/// Half a step rather than a full one: iced snaps a redraw request to the
/// monitor's refresh grid and rounds *up*, so a deadline exactly one period away
/// gets pushed to the frame after it and the model runs at half rate. Half a
/// period lands inside the next refresh, whatever the refresh rate is.
const REDRAW_LEAD: Duration = Duration::from_micros(STEP.as_micros() as u64 / 2);

/// Pixels a scroll notch is assumed to carry, when the platform reports pixels.
const PIXELS_PER_LINE: f32 = 50.0;

/// Starts a model widget.
///
/// ```text
/// live2d(&self.hiyori)
///     .on_event(Message::Live2d)
///     .look_at(look_at::cursor)
///     .width(Fill)
///     .height(Fill)
/// ```
///
/// The widget borrows the model, so the model has to outlive the UI tree — keep
/// it in the host's state. Two widgets may draw one model: both receive its
/// events, each frames it on its own, and each reports the pointer in its own
/// space.
pub fn live2d<Message, I: Interface>(model: &Live2dModel<I>) -> Live2d<'_, Message, I> {
    Live2d::new(model)
}

/// A Live2D model drawn as a widget.
///
/// Only the wgpu renderer works: this delegates to iced's `shader` widget, which
/// the tiny-skia backend cannot draw. Put it into the tree with `column![…]` or
/// [`container`](iced::widget::container), which box it.
///
/// The widget drives its own frames: while the model is not paused it asks for a
/// redraw every animation [`STEP`] and advances the model by the time that
/// actually passed. A host must therefore **not** also subscribe to
/// [`window::frames`] — `window::RedrawRequest` holds a single request and the
/// last writer wins, so the two would fight over the frame rate.
pub struct Live2d<'a, Message, I: Interface> {
    model: &'a Live2dModel<I>,
    on_event: Option<Rc<dyn Fn(Live2dEvent<I>) -> Message + 'a>>,
    look_at: Option<LookAtSource>,
    fill: f32,
    width: Length,
    height: Length,
}

impl<'a, Message, I: Interface> Live2d<'a, Message, I> {
    /// A widget for `model`, filling its container.
    pub fn new(model: &'a Live2dModel<I>) -> Self {
        Self {
            model,
            on_event: None,
            look_at: None,
            fill: DEFAULT_FILL,
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    /// Sends a [`Live2dEvent`] to the host as `Message`.
    ///
    /// Without this, events are dropped: the widget still draws the model, but
    /// load failures, pointer events and finished motions go nowhere, and the
    /// render layer never allocates a channel for them.
    ///
    /// Events arrive one frame after the frame that produced them, and at most one
    /// per frame per widget; a burst is queued, not lost.
    pub fn on_event(mut self, on_event: impl Fn(Live2dEvent<I>) -> Message + 'a) -> Self {
        self.on_event = Some(Rc::new(on_event));
        self
    }

    /// Lets the cursor point the model's head, body and eyes.
    ///
    /// Off by default — pass [`look_at::cursor`](crate::look_at::cursor) for the
    /// usual behaviour, or a function of your own. Once a source is set it owns
    /// the gaze **while it has an answer**: while the cursor is over the widget
    /// that position wins, and while it is not, the model looks at the point set
    /// with [`Live2dModel::look_at`], or straight ahead if there is none.
    ///
    /// Without a source the model is only driven by its own motion and physics,
    /// unless [`Live2dModel::look_at`] was called.
    pub fn look_at(mut self, source: LookAtSource) -> Self {
        self.look_at = Some(source);
        self
    }

    /// How much of the widget the model's bounds fill.
    ///
    /// `1.0` touches the edges; `0.925` is the default the sample models were
    /// framed for. Values that are not positive and finite fall back to `1.0`.
    pub fn fill(mut self, fill: f32) -> Self {
        self.fill = fill;
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

    /// The iced widget that owns the layout, the tree slot and the render pass.
    /// Everything this widget does is delegated to it.
    fn delegate(&self) -> shader::Shader<Message, Self> {
        shader::Shader::new(self.clone())
            .width(self.width)
            .height(self.height)
    }

    /// Queues the input a pointer event carries, and keeps the state the cursor
    /// shape is derived from.
    ///
    /// A release is recorded wherever it happens: dragging out of the widget and
    /// letting go must still end the drag, and the cursor must stop claiming to
    /// hold something.
    fn queue_input(
        &self,
        state: &mut Live2dState,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) {
        let model = self.model.inner();

        match event {
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.pressed = false;
                model.push_pointer(PointerInput::Released);
            }
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if cursor.is_over(bounds) =>
            {
                state.pressed = true;
                model.push_pointer(PointerInput::Pressed);
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / PIXELS_PER_LINE,
                };

                model.push_pointer(PointerInput::Scrolled(lines));
            }
            _ => {}
        }
    }

    /// Takes the next queued event, if the host listens for them.
    fn next_message(&self, state: &mut Live2dState) -> Option<Message> {
        let on_event = self.on_event.as_ref()?;

        state.drain();

        let event = state.pending.pop_front()?;

        let event = match Self::convert(event) {
            Ok(event) => event,
            Err(problem) => Live2dEvent::ModelReportedProblem { problem },
        };

        Some(on_event(event))
    }

    /// Turns a render-layer event into the host's, or into a problem when a
    /// hand-written vocabulary does not cover what the render layer reported.
    fn convert(event: PipelineEvent) -> Result<Live2dEvent<I>, Problem> {
        Ok(match event {
            PipelineEvent::Loaded { info } => Live2dEvent::ModelLoaded { info },
            PipelineEvent::LoadFailed { failure } => Live2dEvent::ModelFailedToLoad { failure },
            PipelineEvent::Problem(problem) => Live2dEvent::ModelReportedProblem { problem },
            PipelineEvent::MotionStarted {
                group,
                clip,
                duration,
                looping,
            } => Live2dEvent::MotionStarted {
                group: key::<I::Motion>(group, "motion")?,
                clip,
                duration,
                looping,
            },
            PipelineEvent::MotionFinished { group, clip } => Live2dEvent::MotionFinishedPlaying {
                group: key::<I::Motion>(group, "motion")?,
                clip,
            },
            PipelineEvent::Pointer(kind) => Live2dEvent::Pointer(match kind {
                PointerEventKind::Pressed { area, at } => PointerEvent::Pressed {
                    area: optional_key::<I::HitArea>(area, "hit area")?,
                    at,
                },
                PointerEventKind::Released { area, at } => PointerEvent::Released {
                    area: optional_key::<I::HitArea>(area, "hit area")?,
                    at,
                },
                PointerEventKind::Entered { area } => PointerEvent::Entered {
                    area: key::<I::HitArea>(area, "hit area")?,
                },
                PointerEventKind::Exited { area } => PointerEvent::Exited {
                    area: key::<I::HitArea>(area, "hit area")?,
                },
                PointerEventKind::Dragged { delta } => PointerEvent::Dragged { delta },
                PointerEventKind::Scrolled { lines } => PointerEvent::Scrolled { lines },
            }),
        })
    }
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

impl<Message, I: Interface> Clone for Live2d<'_, Message, I> {
    fn clone(&self) -> Self {
        Self {
            model: self.model,
            on_event: self.on_event.clone(),
            look_at: self.look_at,
            fill: self.fill,
            width: self.width,
            height: self.height,
        }
    }
}

/// The widget's own slot in the UI tree: one event channel and one view per
/// instance.
pub struct Live2dState {
    events: RefCell<Channel>,
    pending: VecDeque<PipelineEvent>,
    view: ViewId,
    presence: Arc<Presence>,
    hovered: bool,
    pressed: bool,
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

impl Default for Live2dState {
    fn default() -> Self {
        Self {
            events: RefCell::new(Channel::default()),
            pending: VecDeque::new(),
            view: ViewId::unique(),
            presence: Arc::new(Presence),
            hovered: false,
            pressed: false,
        }
    }
}

impl Live2dState {
    fn presence(&self) -> Arc<Presence> {
        Arc::clone(&self.presence)
    }

    /// Moves every queued render-layer event into the widget's own queue, keeping
    /// the hover state the cursor shape is derived from.
    fn drain(&mut self) {
        loop {
            let event = self.events.borrow_mut().try_recv();

            let Some(event) = event else {
                return;
            };

            match &event {
                PipelineEvent::Pointer(PointerEventKind::Entered { .. }) => self.hovered = true,
                PipelineEvent::Pointer(PointerEventKind::Exited { .. }) => self.hovered = false,
                _ => {}
            }

            self.pending.push_back(event);
        }
    }
}

impl<Message, I: Interface> shader::Program<Message> for Live2d<'_, Message, I> {
    type State = Live2dState;
    type Primitive = Live2dPrimitive;

    fn update(
        &self,
        state: &mut Live2dState,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<Message>> {
        self.queue_input(state, event, bounds, cursor);

        let message = self.next_message(state);
        let draining = !state.pending.is_empty();

        let next_frame = match event {
            iced::Event::Window(window::Event::RedrawRequested(now)) if !self.model.is_paused() => {
                Some(*now + REDRAW_LEAD)
            }
            // A burst of events must not wait for the model to animate.
            _ if draining => Some(iced::time::Instant::now()),
            _ => None,
        };

        match (message, next_frame) {
            (Some(message), Some(at)) => {
                Some(shader::Action::publish(message).and_request_redraw_at(at))
            }
            (Some(message), None) => Some(shader::Action::publish(message)),
            (None, Some(at)) => Some(shader::Action::request_redraw_at(at)),
            (None, None) => None,
        }
    }

    fn draw(
        &self,
        state: &Live2dState,
        cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Live2dPrimitive {
        let model = self.model.inner();

        let position = crate::look_at::position(cursor, bounds);
        let look_at = match self.look_at {
            Some(source) => Some(
                source(cursor, bounds)
                    .or_else(|| model.target())
                    .unwrap_or([0.0, 0.0]),
            ),
            None => model.target(),
        };

        let events = if self.on_event.is_some() {
            Some(state.events.borrow_mut().sender())
        } else {
            None
        };

        Live2dPrimitive {
            asset: model.asset(),
            asset_options: model.asset_options(),
            path: model.path_arc(),
            model: model.id(),
            model_options: model.options(),
            view: state.view,
            spec: model.spec(),
            fill: self.fill,
            inputs: FrameInputs {
                parameters: model.parameter_indices(),
                parts: model.part_indices(),
                commands: model
                    .drain_commands()
                    .iter()
                    .map(Command::primitive)
                    .collect(),
                problems: model.take_problems(),
                look_at,
                look_at_mode: model.look_at_mode(),
                physics_enabled: model.physics_enabled(),
                idle: match model.idle() {
                    Idle::Declared => IdleInput::Declared,
                    Idle::Group(group) => IdleInput::Group(group.index()),
                    Idle::Off => IdleInput::Off,
                },
                paused: model.paused(),
                cursor: position,
                pointer: model.drain_pointer(),
            },
            events,
            asset_presence: model.asset_presence(),
            model_presence: model.presence(),
            view_presence: state.presence(),
        }
    }

    fn mouse_interaction(
        &self,
        state: &Live2dState,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.pressed {
            mouse::Interaction::Grabbing
        } else if state.hovered {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::Idle
        }
    }
}

impl<Message, I: Interface> iced::advanced::widget::Meta for Live2d<'_, Message, I> {}

impl<Message, I, Theme, Renderer> Widget<Message, Theme, Renderer> for Live2d<'_, Message, I>
where
    I: Interface,
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
