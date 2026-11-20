//! How host parameter writes are applied.

/// How a host write combines with the value the model produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParameterBlend {
    /// Replaces what the motion, expressions and look-at produced.
    #[default]
    Overwrite,
    /// Added to it; the declared range clamps the result.
    Add,
    /// Multiplied with it; the declared range clamps the result.
    Multiply,
}

/// Where in the step a host write is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParameterStage {
    /// Chosen from the model's own physics data: a parameter physics writes is
    /// applied after physics, everything else before it.
    #[default]
    Auto,
    /// Before physics, so the simulation reads the value. Physics may overwrite a
    /// parameter it owns.
    BeforePhysics,
    /// After physics and before pose, so physics cannot undo the value. Pose still
    /// runs afterwards and keeps its own parameters.
    AfterPhysics,
}

/// One host write.
///
/// `None` fields follow the model's [`ParameterPolicy`], so changing the policy
/// changes every write that did not name its own blend or stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParameterWrite {
    /// The value to overwrite with, add or multiply by, in the parameter's raw
    /// units.
    pub value: f32,
    /// The blend to use, or `None` to follow the policy.
    pub blend: Option<ParameterBlend>,
    /// The stage to use, or `None` to follow the policy.
    pub stage: Option<ParameterStage>,
}

impl ParameterWrite {
    /// An overwriting write that follows the model's policy for everything else.
    pub const fn new(value: f32) -> Self {
        Self {
            value,
            blend: None,
            stage: None,
        }
    }

    /// A write that adds `delta` to whatever the model produced.
    pub const fn add(delta: f32) -> Self {
        Self {
            value: delta,
            blend: Some(ParameterBlend::Add),
            stage: None,
        }
    }

    /// A write that multiplies what the model produced by `factor`.
    pub const fn multiply(factor: f32) -> Self {
        Self {
            value: factor,
            blend: Some(ParameterBlend::Multiply),
            stage: None,
        }
    }

    /// Sets the blend this write uses, overriding the policy.
    pub const fn with_blend(mut self, blend: ParameterBlend) -> Self {
        self.blend = Some(blend);
        self
    }

    /// Sets the stage this write uses, overriding the policy.
    pub const fn at(mut self, stage: ParameterStage) -> Self {
        self.stage = Some(stage);
        self
    }

    pub(crate) fn apply(self, current: f32, policy: ParameterPolicy) -> f32 {
        match self.blend.unwrap_or(policy.blend) {
            ParameterBlend::Overwrite => self.value,
            ParameterBlend::Add => current + self.value,
            ParameterBlend::Multiply => current * self.value,
        }
    }

    pub(crate) fn resolved_stage(
        self,
        policy: ParameterPolicy,
        physics_output: bool,
    ) -> ParameterStage {
        match self.stage.unwrap_or(policy.stage) {
            ParameterStage::Auto if physics_output => ParameterStage::AfterPhysics,
            ParameterStage::Auto => ParameterStage::BeforePhysics,
            stage => stage,
        }
    }
}

/// How a model applies writes that do not name their own blend or stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParameterPolicy {
    /// The blend used by writes that do not name one.
    pub blend: ParameterBlend,
    /// The stage used by writes that do not name one.
    pub stage: ParameterStage,
}

/// How the built-in look-at combines with what the model plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LookAtBlend {
    /// Writes the target pose, replacing what the motion drives.
    #[default]
    Overwrite,
    /// Adds the target to the current value, over half the declared range.
    Add,
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVERWRITE_AUTO: ParameterPolicy = ParameterPolicy {
        blend: ParameterBlend::Overwrite,
        stage: ParameterStage::Auto,
    };

    #[test]
    fn a_write_without_a_blend_follows_the_policy() {
        assert_eq!(ParameterWrite::new(0.5).apply(0.25, OVERWRITE_AUTO), 0.5);
    }

    #[test]
    fn an_explicit_blend_beats_the_policy() {
        let additive = ParameterPolicy {
            blend: ParameterBlend::Add,
            ..OVERWRITE_AUTO
        };

        assert_eq!(ParameterWrite::new(0.5).apply(0.25, additive), 0.75);
        assert_eq!(ParameterWrite::new(0.5).apply(0.25, OVERWRITE_AUTO), 0.5);
    }

    #[test]
    fn add_and_multiply_use_the_current_value() {
        assert_eq!(ParameterWrite::add(0.5).apply(0.25, OVERWRITE_AUTO), 0.75);
        assert_eq!(
            ParameterWrite::multiply(2.0).apply(0.25, OVERWRITE_AUTO),
            0.5
        );
    }

    #[test]
    fn auto_puts_a_physics_output_after_physics() {
        assert_eq!(
            ParameterWrite::new(0.0).resolved_stage(OVERWRITE_AUTO, true),
            ParameterStage::AfterPhysics
        );
    }

    #[test]
    fn auto_puts_every_other_parameter_before_physics() {
        assert_eq!(
            ParameterWrite::new(0.0).resolved_stage(OVERWRITE_AUTO, false),
            ParameterStage::BeforePhysics
        );
    }

    #[test]
    fn an_explicit_stage_beats_auto() {
        let write = ParameterWrite::new(0.0).at(ParameterStage::BeforePhysics);

        assert_eq!(
            write.resolved_stage(OVERWRITE_AUTO, true),
            ParameterStage::BeforePhysics
        );
    }

    #[test]
    fn a_policy_stage_is_followed_until_a_write_names_one() {
        let before = ParameterPolicy {
            stage: ParameterStage::BeforePhysics,
            ..OVERWRITE_AUTO
        };

        assert_eq!(
            ParameterWrite::new(0.0).resolved_stage(before, true),
            ParameterStage::BeforePhysics
        );
    }
}
