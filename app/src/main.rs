//! The demo app: one model, one window, hosted through `iced-live2d`.
//!
//! The app owns exactly two things the component cannot — the asset and the model
//! built from it — and reacts to the events it sends. There is no frame
//! subscription and no iced/wgpu plumbing here: the widget drives its own frames
//! and the render layer lives behind the component.
//!
//! Keys: `R` play a random clip of the next motion group, `E` next expression,
//! `C` clear expressions, `Space` stop the motion, `P` pause/resume. Every pointer
//! event is printed as it arrives.

mod cli;

use std::sync::Arc;

use iced::{Fill, Subscription, Task, Widget, keyboard, window};
use iced_live2d::{
    Live2dAsset, Live2dEvent, Live2dModel, ModelInfo, Raw, live2d, look_at, raw,
};

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
    model: Live2dModel<Raw>,
    /// Filled in when the load event arrives; the key bindings need it to know
    /// what the model offers.
    info: Option<Arc<ModelInfo>>,
    next_group: usize,
    next_expression: usize,
    paused: bool,
}

#[derive(Debug, Clone)]
enum Message {
    Live2d(Live2dEvent<Raw>),
    NextMotion,
    NextExpression,
    ClearExpressions,
    StopMotion,
    TogglePause,
}

impl App {
    fn new(path: std::path::PathBuf, look_at_mode: iced_live2d::LookAtMode) -> Self {
        let asset = Live2dAsset::from_path(path);
        let model = Live2dModel::<Raw>::new(&asset);
        model.set_look_at_mode(look_at_mode);

        Self {
            model,
            info: None,
            next_group: 0,
            next_expression: 0,
            paused: false,
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Live2d(event) => self.report(event),
            Message::NextMotion => self.next_motion(),
            Message::NextExpression => self.next_expression(),
            Message::ClearExpressions => self.model.clear_expressions(),
            Message::StopMotion => self.model.stop_motion(),
            Message::TogglePause => self.toggle_pause(),
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
                "c" => Some(Message::ClearExpressions),
                "s" => Some(Message::StopMotion),
                "p" => Some(Message::TogglePause),
                _ => None,
            },
            _ => None,
        })
    }

    fn report(&mut self, event: Live2dEvent<Raw>) {
        match &event {
            Live2dEvent::ModelLoaded { info } => {
                self.info = Some(Arc::clone(info));

                println!(
                    "live2d: loaded {} — {} motions in {} groups, {} expressions, {} parameters, {} hit areas",
                    self.model.path().display(),
                    info.motion_count(),
                    info.motions.len(),
                    info.expressions.len(),
                    info.parameters.len(),
                    info.hit_areas.len(),
                );
            }
            Live2dEvent::ModelFailedToLoad { failure } => {
                eprintln!("live2d: failed to load {failure}");
            }
            Live2dEvent::ModelReportedProblem { problem } => eprintln!("live2d: {problem}"),
            Live2dEvent::MotionStarted {
                group,
                clip,
                duration,
                looping,
            } => println!(
                "live2d: started {} #{} ({duration:.2} s, looping: {looping})",
                group.name(),
                clip
            ),
            Live2dEvent::MotionFinishedPlaying { group, clip } => {
                println!("live2d: finished {} #{}", group.name(), clip);
            }
            Live2dEvent::Pointer(pointer) => println!("live2d: {pointer:?}"),
        }
    }

    fn next_motion(&mut self) {
        let Some(info) = self.info.as_ref() else {
            return;
        };

        if info.motions.is_empty() {
            return;
        }

        let group = &raw::motions(info)[self.next_group % info.motions.len()];
        self.model.play_random_motion(group.clone());
        self.next_group = self.next_group.wrapping_add(1);
    }

    fn next_expression(&mut self) {
        let Some(info) = self.info.as_ref() else {
            return;
        };

        if info.expressions.is_empty() {
            return;
        }

        let expression = &raw::expressions(info)[self.next_expression % info.expressions.len()];
        self.model.play_expression(expression.clone());
        self.next_expression = self.next_expression.wrapping_add(1);
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;

        if self.paused {
            self.model.pause();
            println!("live2d: paused (no frames, no pointer events)");
        } else {
            self.model.resume();
            println!("live2d: running");
        }
    }
}
