//! The widget.

use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use iced::advanced::layout;
use iced::advanced::widget::tree;
use iced::advanced::{Layout, Widget, renderer};
use iced::widget::shader;
use iced::{Length, Rectangle, mouse, window};
use live2d_render::STEP;

use crate::event::Live2dEvent;
use crate::look_at::LookAtSource;
use crate::model::Live2dModel;
use crate::pipeline::Live2dPrimitive;

/// How much of the widget the model's bounds fill by default.
///
/// The sample models were framed at 1.85 out of the 2.0 NDC range, so this is the
/// framing they were verified against; `.fill(1.0)` touches the edges.
const DEFAULT_FILL: f32 = 0.925;

/// How far ahead of the current frame to ask for the next one.
///
/// Half a step rather than a full one: iced snaps a redraw request to the
/// monitor's refresh grid and rounds *up*, so a deadline exactly one period away
/// gets pushed to the frame after it and the model runs at half rate. Half a
/// period lands inside the next refresh, whatever the refresh rate is.
const REDRAW_LEAD: Duration = Duration::from_micros(STEP.as_micros() as u64 / 2);

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
/// it in the host's state.
pub fn live2d<Message>(model: &Live2dModel) -> Live2d<'_, Message> {
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
pub struct Live2d<'a, Message> {
    model: &'a Live2dModel,
    on_event: Option<Rc<dyn Fn(Live2dEvent) -> Message + 'a>>,
    look_at: Option<LookAtSource>,
    fill: f32,
    width: Length,
    height: Length,
}

impl<'a, Message> Live2d<'a, Message> {
    /// A widget for `model`, filling its container.
    pub fn new(model: &'a Live2dModel) -> Self {
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
    /// load failures and finished motions go nowhere. Events arrive one frame
    /// after the frame that produced them.
    pub fn on_event(mut self, on_event: impl Fn(Live2dEvent) -> Message + 'a) -> Self {
        self.on_event = Some(Rc::new(on_event));
        self
    }

    /// Lets the cursor point the model's head, body and eyes.
    ///
    /// Off by default — pass [`look_at::cursor`](crate::look_at::cursor) for the
    /// usual behaviour, or a function of your own. Once a source is set it owns
    /// the gaze: while it returns a target that target wins, and while it returns
    /// `None` (the cursor is elsewhere) the model looks at the point set with
    /// [`Live2dModel::look_at`], or straight ahead if there is none.
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
    /// framed for. Values that are not positive and finite are ignored, and the
    /// previous framing stays.
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

    /// Takes the next queued event, if the host listens for them.
    fn next_message(&self, state: &mut Live2dState) -> Option<Message> {
        let on_event = self.on_event.as_ref()?;

        while let Ok(event) = state.events.try_recv() {
            state.pending.push_back(event);
        }

        state.pending.pop_front().map(|event| on_event(event))
    }
}

impl<Message> Clone for Live2d<'_, Message> {
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

/// The widget's own slot in the UI tree: one event channel per instance.
#[derive(Debug)]
pub struct Live2dState {
    events: Receiver<Live2dEvent>,
    sender: Sender<Live2dEvent>,
    pending: VecDeque<Live2dEvent>,
}

impl Default for Live2dState {
    fn default() -> Self {
        let (sender, events) = crossbeam_channel::unbounded();

        Self {
            events,
            sender,
            pending: VecDeque::new(),
        }
    }
}

impl<Message> shader::Program<Message> for Live2d<'_, Message> {
    type State = Live2dState;
    type Primitive = Live2dPrimitive;

    fn update(
        &self,
        state: &mut Live2dState,
        event: &iced::Event,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Option<shader::Action<Message>> {
        let next_frame = match event {
            iced::Event::Window(window::Event::RedrawRequested(now)) if !self.model.is_paused() => {
                Some(*now + REDRAW_LEAD)
            }
            _ => None,
        };

        let message = self.next_message(state);

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
        for problem in self.model.take_problems() {
            let _ = state
                .sender
                .send(Live2dEvent::ModelReportedProblem { problem });
        }

        let look_at = match self.look_at {
            Some(source) => Some(
                source(cursor, bounds)
                    .or_else(|| self.model.target())
                    .unwrap_or([0.0, 0.0]),
            ),
            None => self.model.target(),
        };

        Live2dPrimitive {
            id: self.model.id(),
            path: self.model.path_arc(),
            settings: self.model.settings(),
            look_at,
            look_at_mode: self.model.look_at_mode(),
            physics_enabled: self.model.physics_enabled(),
            paused: self.model.is_paused(),
            fill: self.fill,
            commands: self.model.drain_commands(),
            presence: self.model.presence(),
            events: state.sender.clone(),
        }
    }
}

impl<Message> iced::advanced::widget::Meta for Live2d<'_, Message> {}
impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for Live2d<'_, Message>
where
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
