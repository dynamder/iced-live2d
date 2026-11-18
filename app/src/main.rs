//! The demo app: one model, one window, hosted through `iced-live2d`.
//!
//! The app owns exactly one thing the component cannot — the model — and reacts
//! to the events it sends. There is no frame subscription and no iced/wgpu
//! plumbing here: the widget drives its own frames and the render layer lives
//! behind the component.

mod cli;

use iced::{Fill, Subscription, Task, Widget, keyboard, window};
use iced_live2d::{Live2dEvent, Live2dModel, ModelSummary, RenderSettings, live2d, look_at};

fn main() -> iced::Result {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::Invocation::Help) => {
            println!("{}", cli::USAGE);
            return Ok(());
        }
        Ok(cli::Invocation::List) => {
            let models = cli::discover();

            if models.is_empty() {
                println!("no models found; see docs/test-models.md");
            }

            for (name, path) in models {
                println!("{name:<12} {}", path.display());
            }

            return Ok(());
        }
        Ok(cli::Invocation::Run(args)) => args,
        Err(error) => {
            eprintln!("{error}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };

    if !args.model.exists() {
        eprintln!(
            "live2d: model not found at {}\nrun `app --list` to see what is available",
            args.model.display()
        );
    }

    let path = args.model;
    let look_at_mode = args.look_at_mode;

    iced::application(
        move || App::new(path.clone(), look_at_mode),
        App::update,
        App::view,
    )
    .title("Live2D · iced 0.15-dev + mocari")
    .subscription(App::subscription)
    .window(window::Settings {
        size: iced::Size::new(640.0, 640.0),
        ..window::Settings::default()
    })
    .run()
}

struct App {
    model: Live2dModel,
    /// Filled in when the load event arrives; the key bindings need it to know
    /// how many motions and expressions there are.
    summary: Option<ModelSummary>,
    next_motion: usize,
    next_expression: usize,
}

#[derive(Debug, Clone)]
enum Message {
    Live2d(Live2dEvent),
    NextMotion,
    NextExpression,
}

impl App {
    fn new(path: std::path::PathBuf, look_at_mode: iced_live2d::LookAtMode) -> Self {
        let model = Live2dModel::from_path(path, RenderSettings::default());
        model.set_look_at_mode(look_at_mode);

        Self {
            model,
            summary: None,
            next_motion: 0,
            next_expression: 0,
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Live2d(event) => self.report(event),
            Message::NextMotion => self.next_motion(),
            Message::NextExpression => self.next_expression(),
        }

        Task::none()
    }

    fn view(&self) -> impl Widget<Message> {
        live2d(&self.model)
            .on_event(Message::Live2d)
            .look_at(look_at::cursor)
            .width(Fill)
            .height(Fill)
    }

    fn subscription(&self) -> Subscription<Message> {
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
        })
    }

    fn report(&mut self, event: Live2dEvent) {
        match &event {
            Live2dEvent::ModelLoaded { summary } => {
                self.summary = Some(*summary);

                println!(
                    "live2d: loaded {} ({} motions in {} groups, {} expressions)",
                    self.model.path().display(),
                    summary.motions,
                    summary.motion_groups,
                    summary.expressions
                );
            }
            Live2dEvent::ModelFailedToLoad { failure } => eprintln!(
                "live2d: failed to load {}: {}",
                failure.path.display(),
                failure.error
            ),
            Live2dEvent::ModelReportedProblem { problem } => eprintln!("live2d: {problem}"),
            Live2dEvent::MotionFinishedPlaying { motion } => {
                println!("live2d: motion #{} finished", motion.index);
            }
        }
    }

    fn next_motion(&mut self) {
        let Some(summary) = self.summary else {
            return;
        };

        if summary.motions == 0 {
            return;
        }

        self.model.play_motion(self.next_motion % summary.motions);
        self.next_motion = self.next_motion.wrapping_add(1);
    }

    fn next_expression(&mut self) {
        let Some(summary) = self.summary else {
            return;
        };

        if summary.expressions == 0 {
            return;
        }

        self.model
            .play_expression(self.next_expression % summary.expressions);
        self.next_expression = self.next_expression.wrapping_add(1);
    }
}
