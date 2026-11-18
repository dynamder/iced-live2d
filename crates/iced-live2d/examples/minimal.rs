//! Two models in one window, drawn by the component.
//!
//! ```text
//! cargo run -p iced-live2d --example minimal -- <model.model3.json> [<second.model3.json>]
//! ```
//!
//! Keys: `R` next motion, `E` next expression, `P` pause/resume, `1`/`2` drop that
//! model. Every event the component sends is printed and echoed in the status
//! line, so a broken sidecar, an out-of-range motion index or a failed load is
//! visible instead of silent.
//!
//! Dropping a model releases its GPU state at the end of the next rendered frame
//! — watch the process's VRAM in Task Manager / `nvidia-smi` before and after
//! pressing `1`.
//!
//! `LIVE2D_PROFILE=1` prints the render layer's per-frame cost breakdown.

use std::path::PathBuf;

use iced::advanced::Widget as _;
use iced::widget::{column, row, text};
use iced::{Element, Fill, Subscription, Task, keyboard, window};
use iced_live2d::{Live2dEvent, Live2dModel, ModelSummary, RenderSettings, live2d, look_at};

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
    model: Live2dModel,
    /// Filled in when the load event arrives; the key bindings need it to know
    /// how many motions there are.
    summary: Option<ModelSummary>,
    next_motion: usize,
    next_expression: usize,
}

struct App {
    slots: Vec<Option<Slot>>,
    paused: bool,
    status: String,
}

#[derive(Debug, Clone)]
enum Message {
    Live2d(usize, Live2dEvent),
    Key(keyboard::Event),
}

impl App {
    fn new(paths: Vec<PathBuf>) -> Self {
        let slots = paths
            .into_iter()
            .map(|path| {
                Some(Slot {
                    model: Live2dModel::from_path(path, RenderSettings::default()),
                    summary: None,
                    next_motion: 0,
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

    fn on_event(&mut self, index: usize, event: Live2dEvent) {
        let status = match &event {
            Live2dEvent::ModelLoaded { summary } => {
                if let Some(slot) = self.slots.get_mut(index).and_then(Option::as_mut) {
                    slot.summary = Some(*summary);
                }

                format!(
                    "model {} loaded: {} motions in {} groups, {} expressions",
                    index + 1,
                    summary.motions,
                    summary.motion_groups,
                    summary.expressions
                )
            }
            Live2dEvent::ModelFailedToLoad { failure } => format!(
                "model {} failed to load {}: {}",
                index + 1,
                failure.path.display(),
                failure.error
            ),
            Live2dEvent::ModelReportedProblem { problem } => {
                format!("model {}: {problem}", index + 1)
            }
            Live2dEvent::MotionFinishedPlaying { motion } => {
                format!("model {} finished motion #{}", index + 1, motion.index)
            }
        };

        println!("{status}");
        self.status = status;
    }

    fn next_motion(&mut self) {
        for slot in self.slots.iter_mut().flatten() {
            let Some(summary) = slot.summary else {
                continue;
            };

            if summary.motions == 0 {
                continue;
            }

            slot.model.play_motion(slot.next_motion % summary.motions);
            slot.next_motion = slot.next_motion.wrapping_add(1);
        }
    }

    fn next_expression(&mut self) {
        for slot in self.slots.iter_mut().flatten() {
            let Some(summary) = slot.summary else {
                continue;
            };

            if summary.expressions == 0 {
                continue;
            }

            slot.model
                .play_expression(slot.next_expression % summary.expressions);
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
            "paused".to_owned()
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
