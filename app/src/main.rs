mod cli;
mod live2d;

use std::path::PathBuf;
use std::time::Instant;

use iced::keyboard;
use iced::widget::shader;
use iced::window;
use iced::{Fill, Size, Subscription, Task, Widget};
use live2d_render::SceneOptions;

use live2d::{Live2dCommand, Live2dView};

fn main() -> iced::Result {
    let invocation = match cli::parse(std::env::args().skip(1)) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("live2d: {error}\n\n{}", cli::USAGE);
            return Ok(());
        }
    };

    let args = match invocation {
        cli::Invocation::Help => {
            println!("{}", cli::USAGE);
            return Ok(());
        }
        cli::Invocation::List => {
            let models = cli::discover();

            if models.is_empty() {
                println!("no models found; see docs/test-models.md");
            }

            for (name, path) in models {
                println!("{name:<12} {}", path.display());
            }

            return Ok(());
        }
        cli::Invocation::Run(args) => args,
    };

    let model_path = args.model;

    if !model_path.exists() {
        eprintln!(
            "live2d: model not found at {}\nrun `app --list` to see what is available",
            model_path.display()
        );
    }

    let options = SceneOptions {
        tracking: args.tracking,
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
