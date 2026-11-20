//! One layer on top of the first clip, with the write policy switchable at runtime.
//!
//! ```text
//! cargo run --release -p iced-live2d --example policy -- path/to/model.model3.json
//! ```
//!
//! `B` cycles the blend (overwrite, add, multiply), `S` cycles the stage (auto,
//! before physics, after physics), `L` cycles the look-at blend and `P` pauses.
//! The status line names the current policy.
//!
//! On a rig whose mouth feeds physics, switching the stage changes whether the
//! tongue and cheeks follow the layer. On a rig whose clips animate the mouth,
//! switching the blend changes whether the layer replaces or stacks on the clip.
//!
//! The frame subscription is the layer's clock, not its pacing: the widget still
//! asks for the frames it needs.

use iced::advanced::Widget as _;
use iced::time::Instant;
use iced::widget::{column, text};
use iced::{Fill, Subscription, Task, keyboard, window};
use iced_live2d::{
    AssetOptions, Live2dAsset, Live2dEvent, Live2dModel, LookAtBlend, ModelInfo, ModelOptions,
    ParameterBlend, ParameterPolicy, ParameterStage, Raw, RawEvent, live2d, look_at, raw,
};

fn main() -> iced::Result {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: policy <model.model3.json>");
        return Ok(());
    };

    iced::application(
        move || App::new(path.clone().into()),
        App::update,
        App::view,
    )
    .title("iced-live2d — write policy")
    .subscription(App::subscription)
    .window(window::Settings {
        size: iced::Size::new(640.0, 720.0),
        ..window::Settings::default()
    })
    .run()
}

struct App {
    model: Live2dModel<Raw>,
    mouth: Option<raw::RawParameter>,
    breath: Option<raw::RawParameter>,
    blend: ParameterBlend,
    stage: ParameterStage,
    look: LookAtBlend,
    paused: bool,
    started: Instant,
}

#[derive(Debug, Clone)]
enum Message {
    Live2d(RawEvent<Raw>),
    Tick(Instant),
    NextBlend,
    NextStage,
    NextLook,
    TogglePause,
}

impl App {
    fn new(path: std::path::PathBuf) -> Self {
        let asset = Live2dAsset::new(
            path,
            AssetOptions {
                texture_scale: 0.5,
                scan_sidecars: true,
            },
        );

        let model = Live2dModel::<Raw>::new(
            &asset,
            ModelOptions {
                mask_size: 1024,
                parameter_policy: ParameterPolicy {
                    blend: ParameterBlend::Overwrite,
                    stage: ParameterStage::Auto,
                },
                look_at_blend: LookAtBlend::Overwrite,
            },
        );

        Self {
            model,
            mouth: None,
            breath: None,
            blend: ParameterBlend::Overwrite,
            stage: ParameterStage::Auto,
            look: LookAtBlend::Overwrite,
            paused: false,
            started: Instant::now(),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Live2d(event) => self.report(event),
            Message::Tick(now) => self.drive(now),
            Message::NextBlend => self.next_blend(),
            Message::NextStage => self.next_stage(),
            Message::NextLook => self.next_look(),
            Message::TogglePause => self.toggle_pause(),
        }

        Task::none()
    }

    fn report(&mut self, event: RawEvent<Raw>) {
        match event {
            Live2dEvent::ModelLoaded { info, .. } => self.loaded(&info),
            Live2dEvent::ModelFailedToLoad { failure, .. } => eprintln!("live2d: {failure}"),
            Live2dEvent::ModelReportedProblem { problem, .. } => eprintln!("live2d: {problem}"),
            _ => {}
        }
    }

    fn loaded(&mut self, info: &ModelInfo) {
        self.mouth = find(info, "ParamMouthOpenY");
        self.breath = find(info, "ParamBreath");

        if let Some(group) = raw::motions(info).into_iter().next() {
            self.model.play_motion(group, 0);
        }
    }

    fn drive(&mut self, now: Instant) {
        if self.paused {
            return;
        }

        let seconds = now.saturating_duration_since(self.started).as_secs_f32();
        let wave = 0.5 + 0.5 * (seconds * 3.0).sin();

        if let Some(mouth) = &self.mouth {
            self.model.parameters().insert(mouth.clone(), wave);
        }

        if let Some(breath) = &self.breath {
            self.model.parameters().add(breath.clone(), 0.3 * wave);
        }
    }

    fn next_blend(&mut self) {
        self.blend = match self.blend {
            ParameterBlend::Overwrite => ParameterBlend::Add,
            ParameterBlend::Add => ParameterBlend::Multiply,
            ParameterBlend::Multiply => ParameterBlend::Overwrite,
        };

        self.model.parameter_policy_mut().blend = self.blend;
    }

    fn next_stage(&mut self) {
        self.stage = match self.stage {
            ParameterStage::Auto => ParameterStage::BeforePhysics,
            ParameterStage::BeforePhysics => ParameterStage::AfterPhysics,
            ParameterStage::AfterPhysics => ParameterStage::Auto,
        };

        self.model.parameter_policy_mut().stage = self.stage;
    }

    fn next_look(&mut self) {
        self.look = match self.look {
            LookAtBlend::Overwrite => LookAtBlend::Add,
            LookAtBlend::Add => LookAtBlend::Overwrite,
        };

        *self.model.look_at_blend_mut() = self.look;
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;

        if self.paused {
            self.model.pause();
        } else {
            self.model.resume();
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            window::frames().map(Message::Tick),
            keyboard::listen().filter_map(|event| match event {
                keyboard::Event::KeyPressed {
                    text: Some(text),
                    repeat: false,
                    ..
                } => match text.as_str() {
                    "b" => Some(Message::NextBlend),
                    "s" => Some(Message::NextStage),
                    "l" => Some(Message::NextLook),
                    "p" => Some(Message::TogglePause),
                    _ => None,
                },
                _ => None,
            }),
        ])
    }

    fn view(&self) -> impl iced::Widget<Message> {
        let status = format!(
            "blend {:?} · stage {:?} · look {:?}{} — B blend, S stage, L look, P pause",
            self.blend,
            self.stage,
            self.look,
            if self.paused { " · paused" } else { "" },
        );

        column![
            live2d(&self.model)
                .on_event(Message::Live2d)
                .look_at(look_at::cursor)
                .width(Fill)
                .height(Fill)
                ._boxed(),
            text(status),
        ]
    }
}

fn find(info: &ModelInfo, name: &str) -> Option<raw::RawParameter> {
    raw::parameters(info)
        .into_iter()
        .find(|key| key.name() == name)
}
