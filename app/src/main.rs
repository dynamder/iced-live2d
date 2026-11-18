mod live2d;

use std::path::PathBuf;
use std::time::Instant;

use iced::keyboard;
use iced::widget::shader;
use iced::window;
use iced::{Fill, Size, Subscription, Task, Widget};
use live2d_render::{SceneOptions, TrackingMode};

use live2d::{Live2dCommand, Live2dView};

/// Default model, relative to the workspace root. Never committed to git.
const DEFAULT_MODEL: &str = "DeepSeek-fish-live2d/DeepSeek/大肥鱼.model3.json";

fn main() -> iced::Result {
    let model_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_MODEL));

    if !model_path.exists() {
        eprintln!(
            "live2d: model not found at {}\npass a .model3.json path as the first argument",
            model_path.display()
        );
    }

    let options = SceneOptions {
        tracking: tracking_from_env(),
        ..SceneOptions::default()
    };

    iced::application(
        move || App::new(model_path.clone(), options),
        App::update,
        App::view,
    )
    .title("Live2D · iced 0.15-dev + mocari")
    .subscription(App::subscription)
    .window(window::Settings {
        size: Size::new(640.0, 640.0),
        ..window::Settings::default()
    })
    .run()
}

struct App {
    view: Live2dView,
}

#[derive(Debug, Clone)]
enum Message {
    Frame(Instant),
    NextMotion,
    NextExpression,
}

impl App {
    fn new(model_path: PathBuf, options: SceneOptions) -> Self {
        Self {
            view: Live2dView::new(model_path, options),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Frame(now) => self.view.set_now(now),
            // R: next motion clip, E: next expression.
            Message::NextMotion => self.view.send(Live2dCommand::NextMotion),
            Message::NextExpression => self.view.send(Live2dCommand::NextExpression),
        }

        Task::none()
    }

    fn view(&self) -> impl Widget<Message> {
        shader(&self.view).width(Fill).height(Fill)
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            // One message per presented frame; iced keeps requesting redraws for
            // as long as this subscription is alive.
            window::frames().map(Message::Frame),
            keyboard::listen().filter_map(|event| match event {
                keyboard::Event::KeyPressed {
                    text: Some(text),
                    repeat: false,
                    ..
                } => match text.as_str() {
                    "r" => Some(Message::NextMotion),
                    "e" => Some(Message::NextExpression),
                    _ => None,
                },
                _ => None,
            }),
        ])
    }
}

/// `LIVE2D_TRACK=viewer` reproduces the Cubism Viewer's own mouse bindings;
/// anything else (or nothing) resolves the rig's conventions.
fn tracking_from_env() -> TrackingMode {
    let Ok(raw) = std::env::var("LIVE2D_TRACK") else {
        return TrackingMode::default();
    };

    raw.parse().unwrap_or_else(|error| {
        eprintln!("live2d: {error}; using adapted tracking");
        TrackingMode::default()
    })
}
