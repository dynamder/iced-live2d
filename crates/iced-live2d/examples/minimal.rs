//! Two models in one window, driven through the `Raw` interface: any model works
//! without declaring an interface first, and keys are minted from the `ModelInfo`
//! a `ModelLoaded` event carries.
//!
//! ```text
//! cargo run -p iced-live2d --example minimal -- <model.model3.json> [<second.model3.json>]
//! ```
//!
//! Keys: `R` random clip of the next group, `E` next expression, `P` pause/resume,
//! `1`/`2` drop that model — its GPU state goes one frame later. `LIVE2D_PROFILE=1`
//! prints the per-frame cost.

use std::path::PathBuf;
use std::sync::Arc;

use iced::advanced::Widget as _;
use iced::widget::{column, row, text};
use iced::{Element, Fill, Subscription, Task, keyboard, window};
use iced_live2d::{
    AssetOptions, Live2dAsset, Live2dEvent, Live2dModel, ModelInfo, ModelOptions, Raw, live2d,
    look_at, raw,
};

fn main() -> iced::Result {
    let paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();

    if paths.is_empty() {
        eprintln!("usage: minimal <model.model3.json> [<second.model3.json>]");
        eprintln!(
            "example: cargo run -p iced-live2d --example minimal -- path/to/Hiyori.model3.json"
        );
        return Ok(());
    }

    iced::application(move || App::new(paths.clone()), App::update, App::view)
        .title("iced-live2d")
        .subscription(App::subscription)
        .window(window::Settings {
            size: iced::Size::new(1200.0, 640.0),
            ..window::Settings::default()
        })
        .run()
}

struct Slot {
    model: Live2dModel<Raw>,
    /// Filled in by the load event: the key bindings need it to know what the model
    /// offers.
    info: Option<Arc<ModelInfo>>,
    next_group: usize,
    next_expression: usize,
}

struct App {
    slots: Vec<Option<Slot>>,
    paused: bool,
    status: String,
}

#[derive(Debug, Clone)]
enum Message {
    Live2d(usize, Live2dEvent<Raw>),
    Key(keyboard::Event),
}

impl App {
    fn new(paths: Vec<PathBuf>) -> Self {
        let slots = paths
            .into_iter()
            .map(|path| {
                // Opting into the sidecar scan is the host's call.
                let asset = Live2dAsset::new(
                    path,
                    AssetOptions {
                        texture_scale: 0.5,
                        scan_sidecars: true,
                    },
                );
                let model = Live2dModel::<Raw>::new(&asset, ModelOptions::default());

                Some(Slot {
                    model,
                    info: None,
                    next_group: 0,
                    next_expression: 0,
                })
            })
            .collect();

        Self {
            slots,
            paused: false,
            status: "waiting for the models to load".to_owned(),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Live2d(index, event) => self.on_event(index, event),
            Message::Key(keyboard::Event::KeyPressed {
                text: Some(text),
                repeat: false,
                ..
            }) => match text.as_str() {
                "r" => self.next_motion(),
                "e" => self.next_expression(),
                "p" => self.toggle_pause(),
                "1" => self.drop_model(0),
                "2" => self.drop_model(1),
                _ => {}
            },
            Message::Key(_) => {}
        }

        Task::none()
    }

    fn view(&self) -> impl iced::Widget<Message> {
        let mut panes: Vec<Element<'_, Message>> = Vec::with_capacity(self.slots.len());

        for (index, slot) in self.slots.iter().enumerate() {
            match slot {
                Some(slot) => panes.push(
                    live2d(&slot.model)
                        .on_event(move |event| Message::Live2d(index, event))
                        .look_at(look_at::cursor)
                        .profile(std::env::var_os("LIVE2D_PROFILE").is_some())
                        .width(Fill)
                        .height(Fill)
                        ._boxed(),
                ),
                None => panes.push(
                    iced::widget::container(text(format!("model {} dropped", index + 1)))
                        .center(Fill)
                        ._boxed(),
                ),
            }
        }

        column![row(panes).height(Fill), text(self.status.as_str())]
    }

    fn subscription(&self) -> Subscription<Message> {
        keyboard::listen().map(Message::Key)
    }

    fn on_event(&mut self, index: usize, event: Live2dEvent<Raw>) {
        let status = match &event {
            Live2dEvent::ModelLoaded { info } => {
                let start = raw::motions(info).into_iter().next();

                if let Some(slot) = self.slots.get_mut(index).and_then(Option::as_mut) {
                    slot.info = Some(Arc::clone(info));

                    // Nothing plays until the host says so.
                    if let Some(group) = start {
                        slot.model.play_motion(group, 0);
                    }
                }

                format!(
                    "model {} loaded: {} motions in {} groups, {} expressions, {} parameters, {} hit areas",
                    index + 1,
                    info.motion_count(),
                    info.motions.len(),
                    info.expressions.len(),
                    info.parameters.len(),
                    info.hit_areas.len(),
                )
            }
            Live2dEvent::ModelFailedToLoad { failure } => {
                format!("model {} failed to load: {failure}", index + 1)
            }
            Live2dEvent::ModelReportedProblem { problem } => {
                format!("model {}: {problem}", index + 1)
            }
            Live2dEvent::MotionStarted {
                group,
                clip,
                duration,
                looping,
            } => format!(
                "model {} started {} #{} ({duration:.2} s, looping: {looping})",
                index + 1,
                group.name(),
                clip
            ),
            Live2dEvent::MotionFinishedPlaying { group, clip } => {
                // A looping idle never ends; a one-shot does, and the host picks.
                if let Some(slot) = self.slots.get_mut(index).and_then(Option::as_mut) {
                    slot.model.play_random_motion(group.clone());
                }

                format!("model {} finished {} #{}", index + 1, group.name(), clip)
            }
            Live2dEvent::Pointer(event) => format!("model {} {event:?}", index + 1),
        };

        println!("{status}");
        self.status = status;
    }

    fn next_motion(&mut self) {
        for slot in self.slots.iter_mut().flatten() {
            let Some(info) = slot.info.as_ref() else {
                continue;
            };

            if info.motions.is_empty() {
                continue;
            }

            let group = &raw::motions(info)[slot.next_group % info.motions.len()];
            slot.model.play_random_motion(group.clone());
            slot.next_group = slot.next_group.wrapping_add(1);
        }
    }

    fn next_expression(&mut self) {
        for slot in self.slots.iter_mut().flatten() {
            let Some(info) = slot.info.as_ref() else {
                continue;
            };

            if info.expressions.is_empty() {
                continue;
            }

            let expression = &raw::expressions(info)[slot.next_expression % info.expressions.len()];
            slot.model.play_expression(expression.clone());
            slot.next_expression = slot.next_expression.wrapping_add(1);
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;

        for slot in self.slots.iter_mut().flatten() {
            if self.paused {
                slot.model.pause();
            } else {
                slot.model.resume();
            }
        }

        self.status = if self.paused {
            "paused (no frames, no pointer events)".to_owned()
        } else {
            "running".to_owned()
        };
    }

    fn drop_model(&mut self, index: usize) {
        let Some(slot) = self.slots.get_mut(index) else {
            return;
        };

        if slot.take().is_none() {
            return;
        }

        println!("model {} dropped", index + 1);
        self.status = format!("model {} dropped", index + 1);
    }
}
