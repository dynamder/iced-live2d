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
//!
//! Bindings carry a **model parameter index**, resolved once here, so the frame
//! loop never looks a name up.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

/// The six slots a LookAt target drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    /// Head turn left/right.
    HeadYaw,
    /// Head nod up/down.
    HeadPitch,
    /// Body turn left/right.
    BodyYaw,
    /// Body lean towards/away from the viewer.
    BodyLean,
    /// Eye gaze left/right.
    EyeX,
    /// Eye gaze up/down.
    EyeY,
}

impl Slot {
    /// Every slot, in a stable order.
    pub const ALL: [Slot; 6] = [
        Slot::HeadYaw,
        Slot::HeadPitch,
        Slot::BodyYaw,
        Slot::BodyLean,
        Slot::EyeX,
        Slot::EyeY,
    ];

    /// A short name, for diagnostics.
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

    /// Which half of a `-1.0..=1.0` target this slot reads.
    pub fn axis(self) -> Axis {
        match self {
            Slot::HeadYaw | Slot::BodyYaw | Slot::EyeX => Axis::X,
            Slot::HeadPitch | Slot::BodyLean | Slot::EyeY => Axis::Y,
        }
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// The horizontal or vertical half of a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// The horizontal half of a target.
    X,
    /// The vertical half of a target.
    Y,
}

impl Axis {
    /// Picks this axis out of a target.
    /// Picks this axis out of a `-1.0..=1.0` target.
    pub fn of(self, target: [f32; 2]) -> f32 {
        match self {
            Axis::X => target[0],
            Axis::Y => target[1],
        }
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

/// One slot to one model parameter, resolved at load time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Which of the six slots this binding drives.
    pub slot: Slot,
    /// Model parameter index; the model's own spelling is `ModelFacts::name`.
    pub parameter: usize,
    /// How the written value reaches the mesh.
    pub route: Route,
    /// Why this parameter was chosen.
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

/// The resolved look-at table for one model under one [`LookAtMode`].
#[derive(Debug, Clone, Default)]
pub struct Rig {
    bindings: Vec<Binding>,
}

impl Rig {
    /// Every binding, in the order the resolver produced them.
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// The binding for one slot, if the rig drives it.
    pub fn binding(&self, slot: Slot) -> Option<&Binding> {
        self.bindings.iter().find(|binding| binding.slot == slot)
    }

    fn push(&mut self, slot: Slot, parameter: usize, route: Route, source: Source) {
        self.bindings.push(Binding {
            slot,
            parameter,
            route,
            source,
        });
    }
}

/// What the resolver is allowed to look at, so it stays testable without a model.
#[derive(Debug, Clone, Default)]
pub struct ModelFacts {
    /// Parameter ids in model order, so an index stays meaningful.
    parameters: Vec<String>,
    by_name: BTreeMap<String, usize>,
    physics_outputs: BTreeSet<String>,
}

impl ModelFacts {
    /// Builds the facts from the model's parameter ids and its physics outputs.
    pub fn new(
        parameters: impl IntoIterator<Item = String>,
        physics_outputs: impl IntoIterator<Item = String>,
    ) -> Self {
        let parameters: Vec<String> = parameters.into_iter().collect();
        let by_name = parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| (normalize(parameter), index))
            .collect();

        Self {
            parameters,
            by_name,
            physics_outputs: physics_outputs
                .into_iter()
                .map(|name| normalize(&name))
                .collect(),
        }
    }

    /// The model index of a conventional parameter name, if the model has one.
    ///
    /// Matching ignores case and underscores: older rigs use `PARAM_ANGLE_X`
    /// where newer ones use `ParamAngleX`, and both mean the same slot.
    pub fn find(&self, conventional: &str) -> Option<usize> {
        self.by_name.get(&normalize(conventional)).copied()
    }

    /// The model's own spelling of a parameter index.
    pub fn name(&self, index: usize) -> Option<&str> {
        self.parameters.get(index).map(String::as_str)
    }

    /// Whether the model has a parameter under a conventional name.
    pub fn has(&self, conventional: &str) -> bool {
        self.find(conventional).is_some()
    }

    fn is_physics_output(&self, index: usize) -> bool {
        self.parameters
            .get(index)
            .is_some_and(|parameter| self.physics_outputs.contains(&normalize(parameter)))
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

    fn named<'a>(facts: &'a ModelFacts, rig: &Rig, slot: Slot) -> Option<&'a str> {
        let binding = rig.binding(slot)?;
        facts.name(binding.parameter)
    }

    #[test]
    fn adapted_prefers_the_physics_input() {
        let facts = vtuber_rig();
        let rig = resolve(LookAtMode::Adapted, &facts);

        assert_eq!(named(&facts, &rig, Slot::HeadYaw), Some("AngleX"));
        assert_eq!(
            rig.binding(Slot::HeadYaw).unwrap().route,
            Route::ThroughPhysics
        );
        assert_eq!(named(&facts, &rig, Slot::BodyYaw), Some("PositionX"));
        assert_eq!(
            rig.binding(Slot::BodyYaw).unwrap().source,
            Source::PhysicsInput
        );
        assert_eq!(rig.binding(Slot::EyeX).unwrap().route, Route::Direct);
    }

    #[test]
    fn adapted_falls_back_to_the_conventional_name() {
        let facts = classic_rig();
        let rig = resolve(LookAtMode::Adapted, &facts);

        assert_eq!(named(&facts, &rig, Slot::HeadYaw), Some("ParamAngleX"));
        assert_eq!(rig.binding(Slot::HeadYaw).unwrap().route, Route::Direct);
        assert_eq!(named(&facts, &rig, Slot::BodyYaw), Some("ParamBodyAngleX"));
        assert!(rig.binding(Slot::BodyLean).is_none());
    }

    #[test]
    fn viewer_mode_ignores_the_rig_conventions() {
        let facts = vtuber_rig();
        let rig = resolve(LookAtMode::ViewerDefault, &facts);

        assert_eq!(named(&facts, &rig, Slot::HeadYaw), Some("ParamAngleX"));
        assert_eq!(
            rig.binding(Slot::HeadYaw).unwrap().source,
            Source::ViewerDefault
        );
        assert!(rig.binding(Slot::BodyLean).is_none());
    }

    #[test]
    fn a_look_at_input_without_a_conventional_name_is_written_directly() {
        let facts = facts(&["AngleX", "ParamEyeBallX"], &[]);
        let rig = resolve(LookAtMode::Adapted, &facts);

        assert_eq!(named(&facts, &rig, Slot::HeadYaw), Some("AngleX"));
        assert_eq!(rig.binding(Slot::HeadYaw).unwrap().route, Route::Direct);
    }

    #[test]
    fn uppercase_underscore_ids_are_recognised() {
        // Wanko: an older rig with SHOUTING_IDS and no eye-ball parameters.
        let facts = facts(&["PARAM_ANGLE_X", "PARAM_BODY_ANGLE_X"], &[]);
        let rig = resolve(LookAtMode::Adapted, &facts);

        assert_eq!(named(&facts, &rig, Slot::HeadYaw), Some("PARAM_ANGLE_X"));
        assert_eq!(
            named(&facts, &rig, Slot::BodyYaw),
            Some("PARAM_BODY_ANGLE_X")
        );
        assert!(rig.binding(Slot::EyeX).is_none());
    }

    #[test]
    fn missing_parameters_are_skipped() {
        let facts = facts(&["AngleX"], &[]);
        let rig = resolve(LookAtMode::Adapted, &facts);

        assert!(rig.binding(Slot::BodyYaw).is_none());
        assert!(rig.binding(Slot::EyeX).is_none());
    }

    #[test]
    fn every_slot_reads_the_axis_it_names() {
        assert_eq!(Slot::HeadYaw.axis(), Axis::X);
        assert_eq!(Slot::HeadPitch.axis(), Axis::Y);
        assert_eq!(Slot::BodyYaw.axis(), Axis::X);
        assert_eq!(Slot::BodyLean.axis(), Axis::Y);
        assert_eq!(Slot::EyeX.axis(), Axis::X);
        assert_eq!(Slot::EyeY.axis(), Axis::Y);
    }
}
