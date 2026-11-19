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
pub fn live2d<Message, I: Interface>(model: &Live2dModel<I>) -> Live2d<'_, Message, I> {
    Live2d::new(model)
}

/// A Live2D model drawn as a widget. wgpu only; the tiny-skia backend cannot draw
/// iced's `shader` widget. Box it with `column![…]` or `container`.
///
/// The widget drives its own frames. A host must **not** also subscribe to
/// [`window::frames`]: `RedrawRequest` holds one request and the last writer wins.
pub struct Live2d<'a, Message, I: Interface> {
    model: &'a Live2dModel<I>,
    on_event: Option<Rc<dyn Fn(Live2dEvent<I>) -> Message + 'a>>,
    look_at: Option<LookAtSource>,
    fill: f32,
    profile: bool,
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
            profile: false,
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    /// Sends [`Live2dEvent`]s to the host as `Message`.
    ///
    /// Without it events are dropped, load failures included. At most one per
    /// frame, one frame late; a burst is queued, not lost.
    pub fn on_event(mut self, on_event: impl Fn(Live2dEvent<I>) -> Message + 'a) -> Self {
        self.on_event = Some(Rc::new(on_event));
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

    /// The iced widget that owns layout, the tree slot and the render pass.
    fn delegate(&self) -> shader::Shader<Message, Self> {
        shader::Shader::new(self.clone())
            .width(self.width)
            .height(self.height)
    }

    /// Queues the input a pointer event carries.
    ///
    /// A release counts wherever it happens, or dragging out of the widget and
    /// letting go leaves the drag open forever.
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

    /// Turns a render-layer event into the host's, or into a problem when the
    /// vocabulary does not cover what was reported.
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
            profile: self.profile,
            width: self.width,
            height: self.height,
        }
    }
}

/// The widget's own tree slot: one event channel and one view per instance.
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

    /// Moves every queued render-layer event into the widget's queue, noting hover.
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
                paused: model.paused(),
                cursor: position,
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
