//! Classifying raw pointer input into [`Gesture`]s.
//!
//! The whole classifier is a pure state machine over `(PointerEvent, Instant)`: no
//! timers of its own, no allocation beyond a handful of live pointers, and every
//! decision is a function of what it was fed. The widget drives it once per event
//! and once per tick, and asks for the next tick through [`Recognizer::deadline`].

use std::time::Instant;

use iced::Point;

use crate::interaction::{Drag, Gesture, Gestures, Pointer, PointerEvent, Sample, Tap};
use crate::model::Live2dId;

/// A press that has not been released yet.
#[derive(Debug, Clone)]
struct Press<A> {
    model: Live2dId,
    pointer: Pointer,
    area: Option<A>,
    origin: Sample,
    at: Sample,
    started: Instant,
    moved_at: Instant,
    velocity: [f32; 2],
    dragging: bool,
    long_fired: bool,
}

/// Classifies pointer input, per widget.
#[derive(Debug)]
pub(crate) struct Recognizer<A> {
    config: Option<Gestures>,
    presses: Vec<Press<A>>,
    /// The last click, kept for the double-click window.
    clicked: Option<(Instant, Option<A>)>,
}

impl<A> Default for Recognizer<A> {
    fn default() -> Self {
        Self {
            config: None,
            presses: Vec::new(),
            clicked: None,
        }
    }
}

impl<A: Clone + PartialEq> Recognizer<A> {
    /// Sets the tuning, or turns classification off with `None`.
    pub(crate) fn configure(&mut self, config: Option<Gestures>) {
        if self.config != config {
            self.config = config;
            self.presses.clear();
            self.clicked = None;
        }
    }

    /// Feeds one pointer event, appending whatever gestures it completes.
    pub(crate) fn push(
        &mut self,
        event: &PointerEvent<A>,
        now: Instant,
        out: &mut Vec<Gesture<A>>,
    ) {
        let Some(config) = self.config else {
            return;
        };

        match event {
            PointerEvent::Pressed(pointed) => {
                self.presses
                    .retain(|press| press.pointer != pointed.pointer);
                self.presses.push(Press {
                    model: pointed.model,
                    pointer: pointed.pointer,
                    area: pointed.area.clone(),
                    origin: pointed.at,
                    at: pointed.at,
                    started: now,
                    moved_at: now,
                    velocity: [0.0; 2],
                    dragging: false,
                    long_fired: false,
                });
            }
            PointerEvent::Dragged(dragged) => {
                let Some(press) = self
                    .presses
                    .iter_mut()
                    .find(|press| press.pointer == dragged.pointer)
                else {
                    return;
                };

                let seconds = now.saturating_duration_since(press.moved_at).as_secs_f32();

                if seconds > 0.0 {
                    press.velocity = [
                        (dragged.at.pixels.x - press.at.pixels.x) / seconds,
                        (dragged.at.pixels.y - press.at.pixels.y) / seconds,
                    ];
                }

                press.at = dragged.at;
                press.moved_at = now;

                let travelled = distance(press.origin.pixels, dragged.at.pixels);

                if !press.dragging && travelled >= config.drag_threshold {
                    press.dragging = true;

                    out.push(Gesture::DragStarted(drag_of(press)));
                }

                if press.dragging {
                    out.push(Gesture::Dragged(drag_of(press)));
                }
            }
            PointerEvent::Released(released) => {
                let Some(index) = self
                    .presses
                    .iter()
                    .position(|press| press.pointer == released.pointer)
                else {
                    return;
                };

                let press = self.presses.remove(index);
                let held = now.saturating_duration_since(press.started);

                if press.dragging {
                    let mut press = press;
                    press.at = released.at;

                    out.push(Gesture::DragEnded(drag_of(&press)));

                    return;
                }

                if press.long_fired || press.area != released.area {
                    return;
                }

                let tap = Tap {
                    model: press.model,
                    pointer: press.pointer,
                    area: press.area.clone(),
                    at: released.at,
                    held,
                };

                let doubled = self.clicked.as_ref().is_some_and(|(when, area)| {
                    *area == press.area
                        && now.saturating_duration_since(*when) < config.double_click
                });

                if doubled {
                    self.clicked = None;
                    out.push(Gesture::DoubleClicked(tap));
                } else {
                    self.clicked = Some((now, press.area));
                    out.push(Gesture::Clicked(tap));
                }
            }
            PointerEvent::Cancelled(cancelled) => {
                self.presses
                    .retain(|press| press.pointer != cancelled.pointer);
            }
            PointerEvent::Moved(_)
            | PointerEvent::Entered(_)
            | PointerEvent::Exited(_)
            | PointerEvent::Scrolled(_) => {}
        }
    }

    /// Advances the clock, reporting a long press that has come due.
    pub(crate) fn tick(&mut self, now: Instant, out: &mut Vec<Gesture<A>>) {
        let Some(config) = self.config else {
            return;
        };

        for press in &mut self.presses {
            if press.dragging || press.long_fired {
                continue;
            }

            if now.saturating_duration_since(press.started) < config.long_press {
                continue;
            }

            press.long_fired = true;

            out.push(Gesture::LongPressed(Tap {
                model: press.model,
                pointer: press.pointer,
                area: press.area.clone(),
                at: press.origin,
                held: now.saturating_duration_since(press.started),
            }));
        }
    }

    /// When [`Recognizer::tick`] has something to do next, if ever.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        let config = self.config?;

        self.presses
            .iter()
            .filter(|press| !press.dragging && !press.long_fired)
            .map(|press| press.started + config.long_press)
            .min()
    }
}

fn drag_of<A: Clone>(press: &Press<A>) -> Drag<A> {
    Drag {
        model: press.model,
        pointer: press.pointer,
        area: press.area.clone(),
        origin: press.origin,
        at: press.at,
        velocity: press.velocity,
    }
}

fn distance(from: Point, to: Point) -> f32 {
    let dx = to.x - from.x;
    let dy = to.y - from.y;

    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interaction::{Cancelled, Dragging, Hovering, Moving, Pointing};
    use crate::model::Live2dId;
    use iced::mouse;
    use std::time::Duration;

    fn model() -> Live2dId {
        Live2dId::unique()
    }

    fn sample(x: f32) -> Sample {
        Sample {
            pixels: Point::new(x, 0.0),
            clip: [x / 100.0, 0.0],
            model: [x / 100.0, 0.0],
        }
    }

    fn press(area: Option<&'static str>, x: f32) -> PointerEvent<&'static str> {
        PointerEvent::Pressed(Pointing {
            model: model(),
            pointer: Pointer::Mouse(mouse::Button::Left),
            area,
            at: sample(x),
        })
    }

    fn release(area: Option<&'static str>, x: f32) -> PointerEvent<&'static str> {
        PointerEvent::Released(Pointing {
            model: model(),
            pointer: Pointer::Mouse(mouse::Button::Left),
            area,
            at: sample(x),
        })
    }

    fn dragged(x: f32) -> PointerEvent<&'static str> {
        PointerEvent::Dragged(Dragging {
            model: model(),
            pointer: Pointer::Mouse(mouse::Button::Left),
            area: Some("Body"),
            origin: sample(0.0),
            at: sample(x),
        })
    }

    fn tapped() -> Recognizer<&'static str> {
        let mut recognizer = Recognizer::default();
        recognizer.configure(Some(Gestures::default()));

        recognizer
    }

    fn run(
        recognizer: &mut Recognizer<&'static str>,
        events: &[(Instant, PointerEvent<&'static str>)],
    ) -> Vec<Gesture<&'static str>> {
        let mut out = Vec::new();

        for (at, event) in events {
            recognizer.push(event, *at, &mut out);
        }

        out
    }

    #[test]
    fn a_press_and_release_that_stay_put_is_a_click() {
        let mut recognizer = tapped();
        let start = Instant::now();

        let out = run(
            &mut recognizer,
            &[
                (start, press(Some("Body"), 0.0)),
                (
                    start + Duration::from_millis(120),
                    release(Some("Body"), 0.0),
                ),
            ],
        );

        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            Gesture::Clicked(Tap {
                area: Some("Body"),
                held,
                ..
            }) if held == Duration::from_millis(120)
        ));
    }

    #[test]
    fn a_second_click_inside_the_window_is_a_double_click() {
        let mut recognizer = tapped();
        let start = Instant::now();
        let mut events = Vec::new();

        for index in 0..2 {
            let at = start + Duration::from_millis(200 * index);

            events.push((at, press(Some("Body"), 0.0)));
            events.push((at + Duration::from_millis(50), release(Some("Body"), 0.0)));
        }

        let out = run(&mut recognizer, &events);

        assert!(matches!(out[0], Gesture::Clicked(_)));
        assert!(matches!(out[1], Gesture::DoubleClicked(_)));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn a_click_on_another_area_is_not_a_double_click() {
        let mut recognizer = tapped();
        let start = Instant::now();
        let mut events = Vec::new();

        for area in ["Body", "Head"] {
            let at = start + Duration::from_millis(100);

            events.push((at, press(Some(area), 0.0)));
            events.push((at + Duration::from_millis(50), release(Some(area), 0.0)));
        }

        let out = run(&mut recognizer, &events);

        assert!(matches!(out[0], Gesture::Clicked(_)));
        assert!(matches!(out[1], Gesture::Clicked(_)));
    }

    #[test]
    fn holding_still_reports_a_long_press_and_the_release_is_not_a_click() {
        let mut recognizer = tapped();
        let start = Instant::now();
        let mut out = Vec::new();

        recognizer.push(&press(Some("Body"), 0.0), start, &mut out);
        recognizer.tick(start + Duration::from_millis(499), &mut out);
        assert!(out.is_empty());
        assert_eq!(
            recognizer.deadline(),
            Some(start + Duration::from_millis(500))
        );

        recognizer.tick(start + Duration::from_millis(500), &mut out);
        assert!(matches!(out[0], Gesture::LongPressed(_)));

        recognizer.push(
            &release(Some("Body"), 0.0),
            start + Duration::from_millis(600),
            &mut out,
        );

        assert_eq!(out.len(), 1);
        assert!(recognizer.deadline().is_none());
    }

    #[test]
    fn travelling_past_the_threshold_starts_a_drag_and_spends_the_click() {
        let mut recognizer = tapped();
        let start = Instant::now();
        let mut out = Vec::new();

        recognizer.push(&press(Some("Body"), 0.0), start, &mut out);
        recognizer.push(&dragged(4.0), start + Duration::from_millis(16), &mut out);
        assert!(out.is_empty(), "under the threshold is still a press");

        recognizer.push(&dragged(40.0), start + Duration::from_millis(32), &mut out);
        assert!(matches!(out[0], Gesture::DragStarted(_)));
        assert!(matches!(out[1], Gesture::Dragged(_)));

        out.clear();
        recognizer.push(
            &release(Some("Body"), 40.0),
            start + Duration::from_millis(48),
            &mut out,
        );

        match out[0] {
            Gesture::DragEnded(ref drag) => {
                assert_eq!(drag.area, Some("Body"));
                assert!(drag.velocity[0] > 0.0);
                assert_eq!(drag.delta(), [0.4, 0.0]);
            }
            ref other => panic!("expected a drag end, got {other:?}"),
        }

        assert_eq!(out.len(), 1);
    }

    #[test]
    fn releasing_on_another_area_is_not_a_click() {
        let mut recognizer = tapped();
        let start = Instant::now();

        let out = run(
            &mut recognizer,
            &[
                (start, press(Some("Body"), 0.0)),
                (
                    start + Duration::from_millis(50),
                    release(Some("Head"), 2.0),
                ),
            ],
        );

        assert!(out.is_empty());
    }

    #[test]
    fn a_cancelled_press_reports_nothing_and_forgets_the_press() {
        let mut recognizer = tapped();
        let start = Instant::now();
        let mut out = Vec::new();

        recognizer.push(&press(Some("Body"), 0.0), start, &mut out);
        recognizer.push(
            &PointerEvent::Cancelled(Cancelled {
                model: model(),
                pointer: Pointer::Mouse(mouse::Button::Left),
            }),
            start + Duration::from_millis(10),
            &mut out,
        );
        recognizer.push(
            &release(Some("Body"), 0.0),
            start + Duration::from_millis(20),
            &mut out,
        );

        assert!(out.is_empty());
        assert!(recognizer.deadline().is_none());
    }

    #[test]
    fn hover_and_move_do_not_produce_gestures() {
        let mut recognizer = tapped();
        let start = Instant::now();
        let mut out = Vec::new();

        recognizer.push(
            &PointerEvent::Entered(Hovering {
                model: model(),
                area: "Body",
                at: sample(0.0),
            }),
            start,
            &mut out,
        );
        recognizer.push(
            &PointerEvent::Moved(Moving {
                model: model(),
                at: sample(10.0),
            }),
            start,
            &mut out,
        );

        assert!(out.is_empty());
    }

    #[test]
    fn without_a_configuration_nothing_is_classified() {
        let mut recognizer = Recognizer::default();
        let start = Instant::now();
        let mut out = Vec::new();

        recognizer.push(&press(Some("Body"), 0.0), start, &mut out);
        recognizer.push(&release(Some("Body"), 0.0), start, &mut out);
        recognizer.tick(start + Duration::from_secs(5), &mut out);

        assert!(out.is_empty());
        assert!(recognizer.deadline().is_none());
    }
}
