//! The demo app: one model, one window, hosted through `iced-live2d`.
//!
//! What plays is this app's decision — on load it starts the first clip of the
//! first group, and when a clip completes it advances to the next one of that group.
//! There is no frame subscription and no iced/wgpu plumbing here.
//!
//! Keys: `R` random clip of the next group, `E` next expression, `C` clear
//! expressions, `S` stop the motion, `P` pause/resume, `F` cycle the fade: let the
//! model files decide, cut, 250 ms.

mod cli;

use std::sync::Arc;
use std::time::Duration;

use iced::{Fill, Subscription, Task, Widget, keyboard, mouse, window};
use iced_live2d::{
    AssetOptions, Fade, GestureInput, Gestures, Hover, Live2dAsset, Live2dModel, ModelEvent,
    ModelInfo, ModelOptions, MotionClipIndex, MotionEnd, Raw, live2d, look_at, raw,
};

fn main() -> iced::Result {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::Invocation::Help) => {
            println!("{}", cli::USAGE);
            return Ok(());
        }
        Ok(cli::Invocation::Run(args)) => args,
        Err(error) => {
            eprintln!("{error}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };

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
    fade: Option<Fade>,
}

#[derive(Debug, Clone)]
enum Message {
    Live2d(ModelEvent<Raw>),
    Input(GestureInput<Raw>),
    NextMotion,
    NextExpression,
    ClearExpressions,
    StopMotion,
    TogglePause,
    NextFade,
}

impl App {
    fn new(path: std::path::PathBuf, look_at_mode: iced_live2d::LookAtMode) -> Self {
        // Only the app knows whether this folder is a loop set: so it scans.
        let asset = Live2dAsset::new(
            path,
            AssetOptions {
                texture_scale: 0.5,
                scan_sidecars: true,
            },
        );
        let model = Live2dModel::<Raw>::new(&asset, ModelOptions::default());
        model.set_look_at_mode(look_at_mode);

        Self {
            model,
            info: None,
            next_group: 0,
            next_expression: 0,
            paused: false,
            fade: None,
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Live2d(event) => self.report(event),
            Message::Input(input) => println!("live2d: gesture: {input:?}"),
            Message::NextMotion => self.next_motion(),
            Message::NextExpression => self.next_expression(),
            Message::ClearExpressions => self.model.clear_expressions(),
            Message::StopMotion => self.model.stop_motion(),
            Message::TogglePause => self.toggle_pause(),
            Message::NextFade => self.next_fade(),
        }

        Task::none()
    }

    fn view(&self) -> impl Widget<Message> {
        live2d(&self.model)
            // The vocabulary first: it decides the event type the closure below sees.
            .gestures(Gestures::default())
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
                "f" => Some(Message::NextFade),
                _ => None,
            },
            _ => None,
        })
    }

    fn report(&mut self, event: ModelEvent<Raw>) {
        match &event {
            ModelEvent::Loaded { info, .. } => {
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

                // The app, not the component, decides what plays first.
                if let Some(group) = raw::motions(info).into_iter().next() {
                    self.model.play_motion(group, MotionClipIndex::new(0));
                }
            }
            ModelEvent::FailedToLoad { failure, .. } => {
                eprintln!("live2d: failed to load {failure}");
            }
            ModelEvent::Problem { problem, .. } => eprintln!("live2d: {problem}"),
            ModelEvent::MotionStarted {
                group,
                clip,
                duration,
                looping,
                ..
            } => println!(
                "live2d: started {} #{} ({duration:.2} s, looping: {looping})",
                group.name(),
                clip
            ),
            ModelEvent::MotionEnded {
                group, clip, ended, ..
            } => {
                println!(
                    "live2d: {} {} #{}",
                    match ended {
                        MotionEnd::Completed => "finished",
                        MotionEnd::Interrupted => "interrupted",
                    },
                    group.name(),
                    clip
                );

                // Keep the group running in order; the component stops and waits.
                if *ended == MotionEnd::Completed
                    && let Some(info) = self.info.as_ref()
                    && let Some(clips) = info.motions.get(group.index()).map(|item| item.clips)
                    && clips > 0
                {
                    self.model.play_motion(
                        group.clone(),
                        MotionClipIndex::new((clip.get() + 1) % clips),
                    );
                }
            }
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

    fn next_fade(&mut self) {
        self.fade = match self.fade {
            None => Some(Fade::zero()),
            Some(fade) if fade.is_zero() => Some(Fade::from(Duration::from_millis(250))),
            Some(_) => None,
        };
        self.model.set_fade(self.fade);

        println!("live2d: fade {:?}", self.fade);
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
