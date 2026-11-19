//! Every input the component reports, wired to the host's own messages.
//!
//! ```text
//! cargo run -p iced-live2d --example interact -- <model.model3.json> [<another.model3.json>]
//! ```
//!
//! Each pane shows the last thing that happened to it: a click (with how long the
//! press was held), a double click, a long press, a drag with its displacement, a
//! scroll (which zooms that pane) or a hover. Moving the pointer over a model turns
//! its head, so gaze is host-driven here instead of using
//! [`look_at::cursor`](iced_live2d::look_at::cursor).
//!
//! The status line shows the model identity every event carries: with more than one
//! model, the message says which one was touched.
//!
//! This example stays on the **classified** vocabulary: no raw pointer event is
//! delivered, so one physical action produces one message. See `minimal` for raw
//! input.

use std::collections::BTreeMap;
use std::path::PathBuf;

use iced::advanced::Widget as _;
use iced::widget::{column, container, row, text};
use iced::{Element, Fill, Length, Subscription, Task, keyboard, mouse, window};
use iced_live2d::{
    AssetOptions, GestureEvent, GestureMode, Gestures, Hover, Live2d, Live2dAsset, Live2dEvent,
    Live2dId, Live2dModel, ModelOptions, MotionEnd, Raw, live2d, raw,
};

fn main() -> iced::Result {
    let paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();

    if paths.is_empty() {
        eprintln!("usage: interact <model.model3.json> [<another.model3.json>]");
        eprintln!(
            "example: cargo run -p iced-live2d --example interact -- path/to/Hiyori.model3.json"
        );
        return Ok(());
    }

    iced::application(move || App::new(paths.clone()), App::update, App::view)
        .title("iced-live2d — interaction")
        .subscription(App::subscription)
        .window(window::Settings {
            size: iced::Size::new(1200.0, 720.0),
            ..window::Settings::default()
        })
        .run()
}

struct Pane {
    model: Live2dModel<Raw>,
    fill: f32,
    status: String,
}

struct App {
    panes: BTreeMap<Live2dId, Pane>,
    last: String,
    paused: bool,
}

#[derive(Debug, Clone)]
enum Message {
    Live2d(GestureEvent<Raw>),
    Petted {
        model: Live2dId,
        what: String,
    },
    Dragged {
        model: Live2dId,
        at: [f32; 2],
        delta: [f32; 2],
    },
    Turned {
        model: Live2dId,
        at: [f32; 2],
    },
    Zoomed {
        model: Live2dId,
        by: f32,
    },
    Hovered {
        model: Live2dId,
        what: String,
    },
    Key(keyboard::Event),
}

impl App {
    fn new(paths: Vec<PathBuf>) -> Self {
        let mut panes = BTreeMap::new();

        for path in paths {
            let asset = Live2dAsset::new(
                path,
                AssetOptions {
                    texture_scale: 0.5,
                    scan_sidecars: true,
                },
            );
            let model = Live2dModel::<Raw>::new(&asset, ModelOptions::default());
            let id = model.id();

            panes.insert(
                id,
                Pane {
                    model,
                    fill: 0.925,
                    status: "no input yet".to_owned(),
                },
            );
        }

        Self {
            panes,
            last: "click, double click, hold, drag or scroll over a model".to_owned(),
            paused: false,
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Live2d(event) => self.on_event(event),

            Message::Petted { model, what } | Message::Hovered { model, what } => {
                self.report(model, &what);
            }
            Message::Turned { model, at } => {
                if let Some(pane) = self.panes.get(&model) {
                    pane.model.look_at(at);
                }
            }
            Message::Dragged { model, at, delta } => {
                if let Some(pane) = self.panes.get(&model) {
                    pane.model.look_at(at);
                }

                self.report(
                    model,
                    &format!("dragged by [{:.2}, {:.2}]", delta[0], delta[1]),
                );
            }
            Message::Zoomed { model, by } => {
                if let Some(pane) = self.panes.get_mut(&model) {
                    pane.fill = (pane.fill * 1.1_f32.powf(by)).clamp(0.3, 1.0);
                }
            }

            Message::Key(keyboard::Event::KeyPressed {
                text: Some(text),
                repeat: false,
                ..
            }) => {
                if text == " " {
                    self.toggle_pause();
                }
            }
            Message::Key(_) => {}
        }

        Task::none()
    }

    fn view(&self) -> impl iced::Widget<Message> {
        let panes: Vec<Element<'_, Message>> = self
            .panes
            .values()
            .map(|pane| {
                column![
                    container(widget(pane)._boxed()).width(Fill).height(Fill),
                    container(text(pane.status.as_str()).size(13))
                        .height(Length::Fixed(26.0))
                        .width(Fill),
                ]
                .height(Fill)
                .width(Fill)
                ._boxed()
            })
            .collect();

        column![
            row(panes).height(Fill),
            container(text(self.last.as_str()).size(13))
                .height(Length::Fixed(30.0))
                .width(Fill),
        ]
    }

    fn subscription(&self) -> Subscription<Message> {
        keyboard::listen().map(Message::Key)
    }

    fn report(&mut self, model: Live2dId, what: &str) {
        let label = self.label(model);

        if let Some(pane) = self.panes.get_mut(&model) {
            pane.status = what.to_owned();
        }

        self.last = format!("model {label}: {what}");
        println!("{}", self.last);
    }

    fn label(&self, id: Live2dId) -> usize {
        self.panes
            .keys()
            .position(|key| *key == id)
            .map_or(0, |index| index + 1)
    }

    fn on_event(&mut self, event: GestureEvent<Raw>) {
        let model = event.model();
        let label = self.label(model);

        match &event {
            Live2dEvent::ModelLoaded { info, .. } => {
                if let Some(group) = raw::motions(info).into_iter().next()
                    && let Some(pane) = self.panes.get(&model)
                {
                    pane.model.play_motion(group, 0);
                }

                self.last = format!("model {label} loaded");
            }
            Live2dEvent::ModelFailedToLoad { failure, .. } => {
                self.last = format!("model {label} failed to load: {failure}");
                eprintln!("{}", self.last);
            }
            Live2dEvent::ModelReportedProblem { problem, .. } => {
                self.last = format!("model {label}: {problem}");
                eprintln!("{}", self.last);
            }
            Live2dEvent::MotionStarted { group, .. } => {
                self.last = format!("model {label} started {}", group.name());
            }
            Live2dEvent::MotionEnded {
                group,
                ended: MotionEnd::Completed,
                ..
            } => {
                if let Some(pane) = self.panes.get(&model) {
                    pane.model.play_random_motion(group.clone());
                }
            }
            Live2dEvent::MotionEnded { group, .. } => {
                self.last = format!("model {label} interrupted {}", group.name());
            }
            Live2dEvent::Input(_) => {}
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;

        for pane in self.panes.values() {
            if self.paused {
                pane.model.pause();
            } else {
                pane.model.resume();
            }
        }

        self.last = if self.paused {
            "paused — the pose is frozen, input still arrives".to_owned()
        } else {
            "running".to_owned()
        };
    }
}

/// The widget is rebuilt every frame, so every hook is bound here.
fn widget(pane: &Pane) -> Live2d<'_, Message, Raw, GestureMode> {
    live2d(&pane.model)
        .fill(pane.fill)
        .gestures(Gestures::default())
        .on_event(Message::Live2d)
        .cursor(|hover| match hover {
            Hover { pressed: true, .. } => mouse::Interaction::Grabbing,
            Hover { area: Some(_), .. } => mouse::Interaction::Pointer,
            _ => mouse::Interaction::Idle,
        })
        .on_click(|tap| Message::Petted {
            model: tap.model,
            what: match &tap.area {
                Some(area) => format!("clicked {} after {:?}", area.name(), tap.held),
                None => format!("clicked empty space after {:?}", tap.held),
            },
        })
        .on_double_click(|tap| Message::Petted {
            model: tap.model,
            what: format!(
                "double clicked {:?}",
                tap.area.as_ref().map(|area| area.name())
            ),
        })
        .on_long_press(|tap| Message::Petted {
            model: tap.model,
            what: format!(
                "held {:?} for {:?}",
                tap.area.as_ref().map(|area| area.name()),
                tap.held
            ),
        })
        .on_drag(|drag| Message::Dragged {
            model: drag.model,
            at: drag.at.clip,
            delta: drag.delta(),
        })
        .on_drag_end(|drag| Message::Petted {
            model: drag.model,
            what: format!(
                "let go after dragging [{:.2}, {:.2}]",
                drag.delta()[0],
                drag.delta()[1]
            ),
        })
        .on_scroll(|scroll| Message::Zoomed {
            model: scroll.model,
            by: scroll.lines[1],
        })
        .on_move(|moved| Message::Turned {
            model: moved.model,
            at: moved.at.clip,
        })
        .on_enter(|hovered| Message::Hovered {
            model: hovered.model,
            what: format!("over {}", hovered.area.name()),
        })
        .on_exit(|hovered| Message::Hovered {
            model: hovered.model,
            what: format!("left {}", hovered.area.name()),
        })
}
