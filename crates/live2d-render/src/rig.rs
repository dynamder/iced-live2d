//! Where tracking should write, resolved per model instead of hardcoded.
//!
//! Live2D rigs disagree about which parameters a host is supposed to drive. A
//! classic rig exposes `ParamAngleX` and the mesh reads it; a VTuber rig built
//! for VTube Studio exposes `AngleX` as the tracking input and keeps
//! `ParamAngleX` as a *physics output*, so writing `ParamAngleX` is silently
//! undone by the next physics step.
//!
//! [`resolve`] collapses both shapes onto the same six slots. Two policies are
//! available: [`TrackingMode::Adapted`] follows what the rig was authored for,
//! and [`TrackingMode::ViewerDefault`] reproduces the Cubism Viewer's generic
//! mouse bindings exactly, which is what a pixel comparison against the Viewer
//! needs.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

/// The six things a cursor can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    HeadYaw,
    HeadPitch,
    BodyYaw,
    BodyLean,
    EyeX,
    EyeY,
}

impl Slot {
    pub const ALL: [Slot; 6] = [
        Slot::HeadYaw,
        Slot::HeadPitch,
        Slot::BodyYaw,
        Slot::BodyLean,
        Slot::EyeX,
        Slot::EyeY,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Slot::HeadYaw => "head-yaw",
            Slot::HeadPitch => "head-pitch",
            Slot::BodyYaw => "body-yaw",
            Slot::BodyLean => "body-lean",
            Slot::EyeX => "eye-x",
            Slot::EyeY => "eye-y",
        }
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// How a written parameter reaches the mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Physics converts it into the parameter the mesh reads.
    ThroughPhysics,
    /// The mesh reads this parameter directly.
    Direct,
}

impl fmt::Display for Route {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Route::ThroughPhysics => "through-physics",
            Route::Direct => "direct",
        })
    }
}

/// Why a binding was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The conventional name is a physics output, so its input is the real one.
    PhysicsInput,
    /// The conventional name is read by the mesh as is.
    Convention,
    /// Copied from the Cubism Viewer's mouse defaults.
    ViewerDefault,
}

impl fmt::Display for Source {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Source::PhysicsInput => "physics-input",
            Source::Convention => "convention",
            Source::ViewerDefault => "viewer-default",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub slot: Slot,
    pub parameter: String,
    pub route: Route,
    pub source: Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrackingMode {
    /// Follow the rig's own conventions: prefer the physics input when the
    /// conventional parameter is a physics output.
    #[default]
    Adapted,
    /// Reproduce the Cubism Viewer's default mouse bindings, whatever the rig.
    ViewerDefault,
}

impl fmt::Display for TrackingMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            TrackingMode::Adapted => "adapted",
            TrackingMode::ViewerDefault => "viewer",
        })
    }
}

impl FromStr for TrackingMode {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_ascii_lowercase().as_str() {
            "adapted" | "auto" => Ok(TrackingMode::Adapted),
            "viewer" | "reference" => Ok(TrackingMode::ViewerDefault),
            other => Err(format!(
                "tracking mode must be `adapted` or `viewer`, got {other:?}"
            )),
        }
    }
}

/// The resolved tracking table for one model.
#[derive(Debug, Clone, Default)]
pub struct Rig {
    bindings: Vec<Binding>,
}

impl Rig {
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    pub fn binding(&self, slot: Slot) -> Option<&Binding> {
        self.bindings.iter().find(|binding| binding.slot == slot)
    }

    fn push(&mut self, slot: Slot, parameter: &str, route: Route, source: Source) {
        self.bindings.push(Binding {
            slot,
            parameter: parameter.to_owned(),
            route,
            source,
        });
    }
}

/// What the resolver is allowed to look at, so it stays testable without a model.
#[derive(Debug, Clone, Default)]
pub struct ModelFacts {
    parameters: BTreeSet<String>,
    physics_outputs: BTreeSet<String>,
}

impl ModelFacts {
    pub fn new(
        parameters: impl IntoIterator<Item = String>,
        physics_outputs: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            parameters: parameters.into_iter().collect(),
            physics_outputs: physics_outputs.into_iter().collect(),
        }
    }

    pub fn has(&self, parameter: &str) -> bool {
        self.parameters.contains(parameter)
    }

    fn is_physics_output(&self, parameter: &str) -> bool {
        self.physics_outputs.contains(parameter)
    }
}

/// Resolves the tracking table for `facts` under `mode`.
pub fn resolve(mode: TrackingMode, facts: &ModelFacts) -> Rig {
    let mut rig = Rig::default();

    match mode {
        // The Viewer binds the conventional names and nothing else, so on a
        // physics rig these writes are overwritten and only the eyes respond.
        // That is the point: it is the reference behaviour, not a good default.
        TrackingMode::ViewerDefault => {
            for (slot, parameter) in [
                (Slot::HeadYaw, "ParamAngleX"),
                (Slot::HeadPitch, "ParamAngleY"),
                (Slot::BodyYaw, "ParamBodyAngleX"),
            ] {
                if facts.has(parameter) {
                    rig.push(slot, parameter, Route::Direct, Source::ViewerDefault);
                }
            }
        }
        TrackingMode::Adapted => {
            resolve_adapted(&mut rig, facts, Slot::HeadYaw, "AngleX", "ParamAngleX");
            resolve_adapted(&mut rig, facts, Slot::HeadPitch, "AngleY", "ParamAngleY");
            resolve_adapted(
                &mut rig,
                facts,
                Slot::BodyYaw,
                "PositionX",
                "ParamBodyAngleX",
            );
            resolve_adapted(
                &mut rig,
                facts,
                Slot::BodyLean,
                "PositionY",
                "ParamBodyAngleY",
            );
        }
    }

    for (slot, parameter) in [(Slot::EyeX, "ParamEyeBallX"), (Slot::EyeY, "ParamEyeBallY")] {
        if facts.has(parameter) {
            rig.push(slot, parameter, Route::Direct, Source::Convention);
        }
    }

    rig
}

/// Picks between a rig's tracking input and its conventional parameter.
///
/// The interesting case is `ParamAngleX` being a physics output: writing it is
/// useless, and `AngleX` is the parameter the rig actually wants.
fn resolve_adapted(rig: &mut Rig, facts: &ModelFacts, slot: Slot, input: &str, conventional: &str) {
    let conventional_is_output = facts.has(conventional) && facts.is_physics_output(conventional);

    if conventional_is_output {
        if facts.has(input) {
            rig.push(slot, input, Route::ThroughPhysics, Source::PhysicsInput);
        }
    } else if facts.has(conventional) {
        rig.push(slot, conventional, Route::Direct, Source::Convention);
    } else if facts.has(input) {
        rig.push(slot, input, Route::Direct, Source::Convention);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(parameters: &[&str], outputs: &[&str]) -> ModelFacts {
        ModelFacts::new(
            parameters.iter().map(|name| (*name).to_owned()),
            outputs.iter().map(|name| (*name).to_owned()),
        )
    }

    /// The rig in this repository: tracking inputs feed physics outputs.
    fn vtuber_rig() -> ModelFacts {
        facts(
            &[
                "AngleX",
                "AngleY",
                "PositionX",
                "PositionY",
                "ParamAngleX",
                "ParamAngleY",
                "ParamBodyAngleX",
                "ParamBodyAngleY",
                "ParamEyeBallX",
                "ParamEyeBallY",
            ],
            &[
                "ParamAngleX",
                "ParamAngleY",
                "ParamBodyAngleX",
                "ParamBodyAngleY",
            ],
        )
    }

    /// A classic rig with no tracking inputs at all.
    fn classic_rig() -> ModelFacts {
        facts(
            &[
                "ParamAngleX",
                "ParamAngleY",
                "ParamBodyAngleX",
                "ParamEyeBallX",
                "ParamEyeBallY",
            ],
            &[],
        )
    }

    #[test]
    fn adapted_prefers_the_physics_input() {
        let rig = resolve(TrackingMode::Adapted, &vtuber_rig());

        assert_eq!(rig.binding(Slot::HeadYaw).unwrap().parameter, "AngleX");
        assert_eq!(
            rig.binding(Slot::HeadYaw).unwrap().route,
            Route::ThroughPhysics
        );
        assert_eq!(rig.binding(Slot::BodyYaw).unwrap().parameter, "PositionX");
        assert_eq!(
            rig.binding(Slot::BodyYaw).unwrap().source,
            Source::PhysicsInput
        );
        assert_eq!(rig.binding(Slot::EyeX).unwrap().route, Route::Direct);
    }

    #[test]
    fn adapted_falls_back_to_the_conventional_name() {
        let rig = resolve(TrackingMode::Adapted, &classic_rig());

        assert_eq!(rig.binding(Slot::HeadYaw).unwrap().parameter, "ParamAngleX");
        assert_eq!(rig.binding(Slot::HeadYaw).unwrap().route, Route::Direct);
        assert_eq!(
            rig.binding(Slot::BodyYaw).unwrap().parameter,
            "ParamBodyAngleX"
        );
        assert!(rig.binding(Slot::BodyLean).is_none());
    }

    #[test]
    fn viewer_mode_ignores_the_rig_conventions() {
        let rig = resolve(TrackingMode::ViewerDefault, &vtuber_rig());

        assert_eq!(rig.binding(Slot::HeadYaw).unwrap().parameter, "ParamAngleX");
        assert_eq!(
            rig.binding(Slot::HeadYaw).unwrap().source,
            Source::ViewerDefault
        );
        assert!(rig.binding(Slot::BodyLean).is_none());
    }

    #[test]
    fn tracking_input_without_a_conventional_name_is_written_directly() {
        let rig = resolve(
            TrackingMode::Adapted,
            &facts(&["AngleX", "ParamEyeBallX"], &[]),
        );

        assert_eq!(rig.binding(Slot::HeadYaw).unwrap().parameter, "AngleX");
        assert_eq!(rig.binding(Slot::HeadYaw).unwrap().route, Route::Direct);
    }

    #[test]
    fn missing_parameters_are_skipped() {
        let rig = resolve(TrackingMode::Adapted, &facts(&["AngleX"], &[]));

        assert!(rig.binding(Slot::BodyYaw).is_none());
        assert!(rig.binding(Slot::EyeX).is_none());
    }
}
