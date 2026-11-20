//! Two models in one window, driven through the `Raw` interface: any model works
//! without declaring an interface first, and keys are minted from the `ModelInfo`
//! a `Loaded` event carries.
//!
//! This example stays on the **raw** vocabulary: every press, release, drag, hover and
//! scroll is printed with the instant it happened, so a double click shows up as two
//! presses and classifying them is left to the host. See `interact` for the other
//! vocabulary.
//!
//! ```text
//! cargo run -p iced-live2d --example minimal -- <model.model3.json> [<second.model3.json>]
//! ```
//!
//! Keys: `R` random clip of the next group, `E` next expression, `P` pause/resume,
//! `1`/`2` drop the first/second live model. `LIVE2D_PROFILE=1` prints the
//! per-frame cost.
//!
//! Every event carries a [`Live2dId`], so the models are keyed by it and no
//! position in a `Vec` has to be tracked through events.
//!
//! The widget reports model events and interaction separately: `on_model_event`
//! receives the former, `on_event` every raw pointer event.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use iced::advanced::Widget as _;
use iced::widget::{column, row, text};
use iced::{Element, Fill, Subscription, Task, keyboard, mouse, window};
use iced_live2d::{
    AssetOptions, Hover, Live2dAsset, Live2dId, Live2dModel, ModelEvent, ModelInfo, ModelOptions,
    MotionClipIndex, MotionEnd, Raw, RawInput, live2d, look_at, raw,
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
    /// Keyed by the identity every event carries, so iteration order is creation
    /// order and nothing shifts when one is dropped.
    slots: BTreeMap<Live2dId, Slot>,
    paused: bool,
    status: String,
}

#[derive(Debug, Clone)]
enum Message {
    Live2d(ModelEvent<Raw>),
    Input(RawInput<Raw>),
    Key(keyboard::Event),
}

impl App {
    fn new(paths: Vec<PathBuf>) -> Self {
        let mut slots = BTreeMap::new();

        for path in paths {
            // Opting into the sidecar scan is the host's call.
            let asset = Live2dAsset::new(
                path,
                AssetOptions {
                    texture_scale: 0.5,
                    scan_sidecars: true,
                },
            );
            let model = Live2dModel::<Raw>::new(&asset, ModelOptions::default());
            let id = model.id();

            slots.insert(
                id,
                Slot {
                    model,
                    info: None,
                    next_group: 0,
                    next_expression: 0,
                },
            );
        }

        Self {
            slots,
            paused: false,
            status: "waiting for the models to load".to_owned(),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Live2d(event) => self.on_model(event),
            Message::Input(input) => self.on_input(input),
            Message::Key(keyboard::Event::KeyPressed {
                text: Some(text),
                repeat: false,
                ..
            }) => match text.as_str() {
                "r" => self.next_motion(),
                "e" => self.next_expression(),
                "p" => self.toggle_pause(),
                "1" => self.drop_nth(0),
                "2" => self.drop_nth(1),
                _ => {}
            },
            Message::Key(_) => {}
        }

        Task::none()
    }

    fn view(&self) -> impl iced::Widget<Message> {
        let panes: Vec<Element<'_, Message>> = if self.slots.is_empty() {
            vec![
                iced::widget::container(text("every model was dropped"))
                    .center(Fill)
                    ._boxed(),
            ]
        } else {
            self.slots
                .values()
                .map(|slot| {
                    live2d(&slot.model)
                        .raw()
                        .on_event(Message::Input)
                        .on_model_event(Message::Live2d)
                        .look_at(look_at::cursor)
                        .cursor(|hover| match hover {
                            Hover { pressed: true, .. } => mouse::Interaction::Grabbing,
                            Hover { area: Some(_), .. } => mouse::Interaction::Pointer,
                            _ => mouse::Interaction::Idle,
                        })
                        .profile(std::env::var_os("LIVE2D_PROFILE").is_some())
                        .width(Fill)
                        .height(Fill)
                        ._boxed()
                })
                .collect()
        };

        column![row(panes).height(Fill), text(self.status.as_str())]
    }

    fn subscription(&self) -> Subscription<Message> {
        keyboard::listen().map(Message::Key)
    }

    /// Where a model sits in the live set, for the status line only.
    fn label(&self, id: Live2dId) -> usize {
        self.slots
            .keys()
            .position(|key| *key == id)
            .map_or(0, |index| index + 1)
    }

    fn on_input(&mut self, input: RawInput<Raw>) {
        let status = format!("model {} pointer {input:?}", self.label(input.model()));

        println!("{status}");
        self.status = status;
    }

    fn on_model(&mut self, event: ModelEvent<Raw>) {
        let id = event.model();
        let label = self.label(id);

        let status = match &event {
            ModelEvent::Loaded { info, .. } => {
                let start = raw::motions(info).into_iter().next();

                if let Some(slot) = self.slots.get_mut(&id) {
                    slot.info = Some(Arc::clone(info));

                    // Nothing plays until the host says so.
                    if let Some(group) = start {
                        slot.model.play_motion(group, MotionClipIndex::new(0));
                    }
                }

                format!(
                    "model {label} loaded: {} motions in {} groups, {} expressions, {} parameters, {} hit areas",
                    info.motion_count(),
                    info.motions.len(),
                    info.expressions.len(),
                    info.parameters.len(),
                    info.hit_areas.len(),
                )
            }
            ModelEvent::FailedToLoad { failure, .. } => {
                format!("model {label} failed to load: {failure}")
            }
            ModelEvent::Problem { problem, .. } => {
                format!("model {label}: {problem}")
            }
            ModelEvent::MotionStarted {
                group,
                clip,
                duration,
                looping,
                ..
            } => format!(
                "model {label} started {} #{} ({duration:.2} s, looping: {looping})",
                group.name(),
                clip
            ),
            ModelEvent::MotionEnded {
                group,
                clip,
                ended: MotionEnd::Completed,
                ..
            } => {
                // A looping idle never ends; a one-shot does, and the host picks.
                if let Some(slot) = self.slots.get_mut(&id) {
                    slot.model.play_random_motion(group.clone());
                }

                format!("model {label} finished {} #{}", group.name(), clip)
            }
            ModelEvent::MotionEnded { group, clip, .. } => {
                format!("model {label} interrupted {} #{}", group.name(), clip)
            }
        };

        println!("{status}");
        self.status = status;
    }

    fn next_motion(&mut self) {
        for slot in self.slots.values_mut() {
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
        for slot in self.slots.values_mut() {
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

        for slot in self.slots.values_mut() {
            if self.paused {
                slot.model.pause();
            } else {
                slot.model.resume();
            }
        }

        self.status = if self.paused {
            "paused (nothing steps; pointer events still arrive)".to_owned()
        } else {
            "running".to_owned()
        };
    }

    fn drop_nth(&mut self, nth: usize) {
        let Some(id) = self.slots.keys().nth(nth).copied() else {
            return;
        };

        self.slots.remove(&id);

        let status = format!("model {} dropped", nth + 1);
        println!("{status}");
        self.status = status;
    }
}
