//! Where a LookAt target should be written, resolved per model instead of
//! hardcoded.
//!
//! Live2D rigs disagree about which parameters a host is supposed to drive. A
//! classic rig exposes `ParamAngleX` and the mesh reads it; a VTuber rig built
//! for VTube Studio exposes `AngleX` as the look-at input and keeps
//! `ParamAngleX` as a *physics output*, so writing `ParamAngleX` is silently
//! undone by the next physics step.
//!
//! [`resolve`] collapses both shapes onto the same six slots. Two policies are
//! available: [`LookAtMode::Adapted`] follows what the rig was authored for,
//! and [`LookAtMode::ViewerDefault`] reproduces the Cubism Viewer's generic
//! mouse bindings exactly, which is what a pixel comparison against the Viewer
//! needs.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

/// The six slots a LookAt target drives.
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

/// Which parameters a LookAt target is written to.
///
/// Cubism's `CubismLookController` has the same switch under the name *blend
/// mode*: it decides who receives the target, not where the target comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LookAtMode {
    /// Follow the rig's own conventions: prefer the look-at input when the
    /// conventional parameter is a physics output.
    #[default]
    Adapted,
    /// Reproduce the Cubism Viewer's default mouse bindings, whatever the rig.
    ViewerDefault,
}

impl fmt::Display for LookAtMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            LookAtMode::Adapted => "adapted",
            LookAtMode::ViewerDefault => "viewer",
        })
    }
}

impl FromStr for LookAtMode {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_ascii_lowercase().as_str() {
            "adapted" | "auto" => Ok(LookAtMode::Adapted),
            "viewer" | "reference" => Ok(LookAtMode::ViewerDefault),
            other => Err(format!(
                "the look-at mode must be `adapted` or `viewer`, got {other:?}"
            )),
        }
    }
}

/// The resolved look-at table for one model.
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
    parameters: BTreeMap<String, String>,
    physics_outputs: BTreeSet<String>,
}

impl ModelFacts {
    pub fn new(
        parameters: impl IntoIterator<Item = String>,
        physics_outputs: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            parameters: parameters
                .into_iter()
                .map(|parameter| (normalize(&parameter), parameter))
                .collect(),
            physics_outputs: physics_outputs
                .into_iter()
                .map(|name| normalize(&name))
                .collect(),
        }
    }

    /// The model's own spelling of a conventional parameter name, if it has one.
    ///
    /// Matching ignores case and underscores: older rigs use `PARAM_ANGLE_X`
    /// where newer ones use `ParamAngleX`, and both mean the same slot.
    pub fn find(&self, conventional: &str) -> Option<&str> {
        self.parameters
            .get(&normalize(conventional))
            .map(String::as_str)
    }

    pub fn has(&self, conventional: &str) -> bool {
        self.find(conventional).is_some()
    }

    fn is_physics_output(&self, parameter: &str) -> bool {
        self.physics_outputs.contains(&normalize(parameter))
    }
}

/// Case and underscore insensitive key for a parameter name.
fn normalize(parameter: &str) -> String {
    parameter
        .chars()
        .filter(|character| *character != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Resolves the look-at table for `facts` under `mode`.
pub fn resolve(mode: LookAtMode, facts: &ModelFacts) -> Rig {
    let mut rig = Rig::default();

    match mode {
        // The Viewer binds the conventional names and nothing else, so on a
        // physics rig these writes are overwritten and only the eyes respond.
        // That is the point: it is the reference behaviour, not a good default.
        LookAtMode::ViewerDefault => {
            for (slot, parameter) in [
                (Slot::HeadYaw, "ParamAngleX"),
                (Slot::HeadPitch, "ParamAngleY"),
                (Slot::BodyYaw, "ParamBodyAngleX"),
            ] {
                if let Some(parameter) = facts.find(parameter) {
                    rig.push(slot, parameter, Route::Direct, Source::ViewerDefault);
                }
            }
        }
        LookAtMode::Adapted => {
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

    let eye_source = match mode {
        LookAtMode::Adapted => Source::Convention,
        LookAtMode::ViewerDefault => Source::ViewerDefault,
    };

    for (slot, parameter) in [(Slot::EyeX, "ParamEyeBallX"), (Slot::EyeY, "ParamEyeBallY")] {
        if let Some(parameter) = facts.find(parameter) {
            rig.push(slot, parameter, Route::Direct, eye_source);
        }
    }

    rig
}

/// Picks between a rig's own look-at input and its conventional parameter.
///
/// The interesting case is `ParamAngleX` being a physics output: writing it is
/// useless, and `AngleX` is the parameter the rig actually wants.
fn resolve_adapted(rig: &mut Rig, facts: &ModelFacts, slot: Slot, input: &str, conventional: &str) {
    let conventional = facts.find(conventional);
    let input = facts.find(input);

    if let Some(conventional) = conventional {
        if facts.is_physics_output(conventional) {
            if let Some(input) = input {
                rig.push(slot, input, Route::ThroughPhysics, Source::PhysicsInput);
            }
        } else {
            rig.push(slot, conventional, Route::Direct, Source::Convention);
        }
    } else if let Some(input) = input {
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

    /// The rig in this repository: look-at inputs feed physics outputs.
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

    /// A classic rig with no look-at inputs at all.
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
        let rig = resolve(LookAtMode::Adapted, &vtuber_rig());

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
        let rig = resolve(LookAtMode::Adapted, &classic_rig());

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
        let rig = resolve(LookAtMode::ViewerDefault, &vtuber_rig());

        assert_eq!(rig.binding(Slot::HeadYaw).unwrap().parameter, "ParamAngleX");
        assert_eq!(
            rig.binding(Slot::HeadYaw).unwrap().source,
            Source::ViewerDefault
        );
        assert!(rig.binding(Slot::BodyLean).is_none());
    }

    #[test]
    fn a_look_at_input_without_a_conventional_name_is_written_directly() {
        let rig = resolve(
            LookAtMode::Adapted,
            &facts(&["AngleX", "ParamEyeBallX"], &[]),
        );

        assert_eq!(rig.binding(Slot::HeadYaw).unwrap().parameter, "AngleX");
        assert_eq!(rig.binding(Slot::HeadYaw).unwrap().route, Route::Direct);
    }

    #[test]
    fn uppercase_underscore_ids_are_recognised() {
        // Wanko: an older rig with SHOUTING_IDS and no eye-ball parameters.
        let rig = resolve(
            LookAtMode::Adapted,
            &facts(&["PARAM_ANGLE_X", "PARAM_BODY_ANGLE_X"], &[]),
        );

        assert_eq!(
            rig.binding(Slot::HeadYaw).unwrap().parameter,
            "PARAM_ANGLE_X"
        );
        assert_eq!(
            rig.binding(Slot::BodyYaw).unwrap().parameter,
            "PARAM_BODY_ANGLE_X"
        );
        assert!(rig.binding(Slot::EyeX).is_none());
    }

    #[test]
    fn missing_parameters_are_skipped() {
        let rig = resolve(LookAtMode::Adapted, &facts(&["AngleX"], &[]));

        assert!(rig.binding(Slot::BodyYaw).is_none());
        assert!(rig.binding(Slot::EyeX).is_none());
    }
}
