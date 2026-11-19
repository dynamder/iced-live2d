//! A typed interface, declared by the host, driving one model. The enums below are
//! the whole interface: their names are what the model must declare, and a model
//! lacking any of them is rejected at load with the missing names listed.
//!
//! ```text
//! cargo run -p iced-live2d --features macros --example typed -- <model.model3.json>
//! ```
//!
//! Try it on a model with other conventions
//! (`.refs/cubism-web-samples/Samples/Resources/Wanko/Wanko.model3.json`) to see
//! the rejection. Keys: `M` mouth, `C` cheeks, `H` arm, `T` play `TapBody`,
//! `S` stop, `R` restart `Idle`; clicking the `Body` hit area plays `TapBody`.

use std::path::PathBuf;
use std::sync::Arc;

use iced::advanced::Widget as _;
use iced::widget::{column, text};
use iced::{Element, Fill, Subscription, Task, keyboard, mouse, window};
use iced_live2d::{
    AssetOptions, Hover, Interface, Live2dAsset, Live2dEvent, Live2dModel, ModelInfo, ModelOptions,
    PointerEvent, Vocabulary, live2d, look_at,
};

/// The parameters this interface drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
#[vocabulary(prefix = "Param")]
enum FaceParameter {
    AngleX,
    AngleY,
    EyeBallX,
    EyeBallY,
    EyeLOpen,
    EyeROpen,
    MouthOpenY,
    Cheek,
}

/// The parts it may hide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
enum FacePart {
    #[name("PartArmA")]
    ArmA,
    #[name("PartArmB")]
    ArmB,
}

/// The motion groups it may play.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
enum FaceMotion {
    Idle,
    TapBody,
}

/// Hiyori declares no expressions, so this vocabulary has no variants: no key can
/// exist, which is exactly the capability the model has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
enum FaceExpression {}

/// The hit areas it reports. Naming one Hiyori lacks would fail the load.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
enum FaceHitArea {
    Body,
}

struct Face;

impl Interface for Face {
    type Parameter = FaceParameter;
    type Part = FacePart;
    type Motion = FaceMotion;
    type Expression = FaceExpression;
    type HitArea = FaceHitArea;
}

fn main() -> iced::Result {
    let path = match std::env::args().nth(1) {
        Some(path) => PathBuf::from(path),
        None => {
            eprintln!("usage: typed <model.model3.json>");
            return Ok(());
        }
    };

    iced::application(move || App::new(path.clone()), App::update, App::view)
        .title("iced-live2d · typed interface")
        .subscription(App::subscription)
        .window(window::Settings {
            size: iced::Size::new(700.0, 700.0),
            ..window::Settings::default()
        })
        .run()
}

struct App {
    model: Live2dModel<Face>,
    info: Option<Arc<ModelInfo>>,
    mouth_open: bool,
    cheeks: bool,
    arm_visible: bool,
    status: String,
}

#[derive(Debug, Clone)]
enum Message {
    Live2d(Live2dEvent<Face>),
    Key(keyboard::Event),
}

impl App {
    fn new(path: PathBuf) -> Self {
        let asset = Live2dAsset::new(path, AssetOptions::default());
        let model = Live2dModel::<Face>::new(&asset, ModelOptions::default());

        Self {
            model,
            info: None,
            mouth_open: false,
            cheeks: false,
            arm_visible: true,
            status: "waiting for the model".to_owned(),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Live2d(event) => self.on_event(event),
            Message::Key(keyboard::Event::KeyPressed {
                text: Some(text),
                repeat: false,
                ..
            }) => match text.as_str() {
                "m" => self.toggle_mouth(),
                "c" => self.toggle_cheeks(),
                "h" => self.toggle_arm(),
                "t" => self.model.play_motion(FaceMotion::TapBody, 0),
                "s" => self.model.stop_motion(),
                "r" => self.model.play_motion(FaceMotion::Idle, 0),
                _ => {}
            },
            Message::Key(_) => {}
        }

        Task::none()
    }

    fn view(&self) -> impl iced::Widget<Message> {
        let pane: Element<'_, Message> = live2d(&self.model)
            .on_event(Message::Live2d)
            .look_at(look_at::cursor)
            .cursor(|hover| match hover {
                Hover { pressed: true, .. } => mouse::Interaction::Grabbing,
                Hover { area: Some(_), .. } => mouse::Interaction::Pointer,
                _ => mouse::Interaction::Idle,
            })
            .profile(std::env::var_os("LIVE2D_PROFILE").is_some())
            .width(Fill)
            .height(Fill)
            ._boxed();

        column![pane, text(self.status.as_str())]
    }

    fn subscription(&self) -> Subscription<Message> {
        keyboard::listen().map(Message::Key)
    }

    fn on_event(&mut self, event: Live2dEvent<Face>) {
        match &event {
            Live2dEvent::ModelLoaded { info } => {
                self.info = Some(Arc::clone(info));

                // Playing `Idle` is this example's policy, not the component's.
                self.model.play_motion(FaceMotion::Idle, 0);
                self.status = format!(
                    "loaded: {} parameters, {} parts, {} motion groups, {} expressions, {} hit areas",
                    info.parameters.len(),
                    info.parts.len(),
                    info.motions.len(),
                    info.expressions.len(),
                    info.hit_areas.len(),
                );
            }
            Live2dEvent::ModelFailedToLoad { failure } => {
                self.status = format!("failed to load: {failure}");
            }
            Live2dEvent::ModelReportedProblem { problem } => {
                self.status = problem.to_string();
            }
            Live2dEvent::MotionStarted {
                group,
                clip,
                duration,
                looping,
            } => {
                self.status =
                    format!("started {group:?} #{clip} ({duration:.2} s, looping: {looping})");
            }
            Live2dEvent::MotionFinishedPlaying { group, clip } => {
                // Back to the first idle clip, which is this example's policy.
                if *group == FaceMotion::Idle {
                    self.model.play_motion(FaceMotion::Idle, 0);
                }

                self.status = format!("finished {group:?} #{clip}");
            }
            Live2dEvent::Pointer(PointerEvent::Pressed {
                area: Some(FaceHitArea::Body),
                ..
            }) => {
                self.model.play_motion(FaceMotion::TapBody, 0);
                self.status = "pressed Body: TapBody".to_owned();
            }
            Live2dEvent::Pointer(pointer) => {
                self.status = format!("{pointer:?}");
            }
        }

        println!("{}", self.status);
    }

    fn toggle_mouth(&mut self) {
        self.mouth_open = !self.mouth_open;

        let value = if self.mouth_open {
            self.maximum(FaceParameter::MouthOpenY)
        } else {
            0.0
        };

        self.model
            .parameters()
            .insert(FaceParameter::MouthOpenY, value);
        self.status = format!("mouth open: {} ({value})", self.mouth_open);
    }

    fn toggle_cheeks(&mut self) {
        self.cheeks = !self.cheeks;

        let value = if self.cheeks {
            self.maximum(FaceParameter::Cheek)
        } else {
            0.0
        };

        self.model.parameters().insert(FaceParameter::Cheek, value);
        self.status = format!("cheeks: {} ({value})", self.cheeks);
    }

    fn toggle_arm(&mut self) {
        self.arm_visible = !self.arm_visible;

        let opacity = if self.arm_visible { 1.0 } else { 0.0 };
        self.model.parts().insert(FacePart::ArmA, opacity);
        self.status = format!("arm visible: {}", self.arm_visible);
    }

    /// The model's own ceiling for a parameter. Values are raw, so "as open as this
    /// rig goes" is a question for `ModelInfo`, not `1.0`.
    fn maximum(&self, key: FaceParameter) -> f32 {
        let Some(info) = self.info.as_ref() else {
            return 1.0;
        };

        let Some(index) = info.parameter(key.name()) else {
            return 1.0;
        };

        info.parameters
            .get(index)
            .map_or(1.0, |range| range.maximum)
    }
}
