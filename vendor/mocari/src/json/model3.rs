use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{Error, Result};

const FORMAT: &str = "model3.json";
const SUPPORTED_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq)]
/// Parsed Cubism `model3.json` data.
///
/// The model file is the manifest for a Live2D model folder. It points to the
/// `.moc3` file, textures, motions, expressions, physics, pose data, display
/// metadata, groups, and hit areas.
pub struct Model3 {
    version: u32,
    file_references: FileReferences,
    groups: Vec<Group>,
    hit_areas: Vec<HitArea>,
}

impl Model3 {
    /// Parses a `model3.json` document from a string.
    pub fn from_json_str(source: &str) -> Result<Self> {
        let raw: RawModel3 = serde_json::from_str(source).map_err(|error| Error::InvalidJson {
            format: FORMAT,
            message: error.to_string(),
        })?;

        if raw.version != SUPPORTED_VERSION {
            return Err(Error::UnsupportedVersion {
                format: FORMAT,
                version: raw.version,
            });
        }

        Ok(Self {
            version: raw.version,
            file_references: raw.file_references,
            groups: raw.groups,
            hit_areas: raw.hit_areas,
        })
    }

    /// Returns the supported Cubism model version.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Returns the referenced `.moc3` path relative to the model file.
    pub fn moc(&self) -> &str {
        &self.file_references.moc
    }

    /// Returns texture paths relative to the model file.
    pub fn textures(&self) -> &[String] {
        &self.file_references.textures
    }

    /// Returns the optional `physics3.json` path.
    pub fn physics(&self) -> Option<&str> {
        self.file_references.physics.as_deref()
    }

    /// Returns the optional `pose3.json` path.
    pub fn pose(&self) -> Option<&str> {
        self.file_references.pose.as_deref()
    }

    /// Returns the optional `cdi3.json` path.
    pub fn display_info(&self) -> Option<&str> {
        self.file_references.display_info.as_deref()
    }

    /// Returns motion groups keyed by their Cubism group name.
    pub fn motions(&self) -> &BTreeMap<String, Vec<MotionReference>> {
        &self.file_references.motions
    }

    /// Returns expression references declared by the model.
    pub fn expressions(&self) -> &[ExpressionReference] {
        &self.file_references.expressions
    }

    /// Returns parameter and part groups declared by the model.
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// Returns named hit areas declared by the model.
    pub fn hit_areas(&self) -> &[HitArea] {
        &self.hit_areas
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct RawModel3 {
    #[serde(rename = "Version")]
    version: u32,
    #[serde(rename = "FileReferences")]
    file_references: FileReferences,
    #[serde(rename = "Groups", default)]
    groups: Vec<Group>,
    #[serde(rename = "HitAreas", default)]
    hit_areas: Vec<HitArea>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct FileReferences {
    #[serde(rename = "Moc")]
    moc: String,
    #[serde(rename = "Textures")]
    textures: Vec<String>,
    #[serde(rename = "Physics", default)]
    physics: Option<String>,
    #[serde(rename = "Pose", default)]
    pose: Option<String>,
    #[serde(rename = "DisplayInfo", default)]
    display_info: Option<String>,
    #[serde(rename = "Motions", default)]
    motions: BTreeMap<String, Vec<MotionReference>>,
    #[serde(rename = "Expressions", default)]
    expressions: Vec<ExpressionReference>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
/// Reference to a motion file from `model3.json`.
///
/// A reference may also carry the motion's fade times. Those override the ones in
/// the motion file itself, which is how most rigs ship them; `None` means the model
/// file says nothing and the motion file decides.
pub struct MotionReference {
    #[serde(rename = "File")]
    file: String,
    #[serde(rename = "FadeInTime", default)]
    fade_in_time: Option<f32>,
    #[serde(rename = "FadeOutTime", default)]
    fade_out_time: Option<f32>,
}

impl MotionReference {
    /// Returns the motion file path relative to the model file.
    pub fn file(&self) -> &str {
        &self.file
    }

    /// Returns the fade-in this reference declares, in seconds.
    ///
    /// A negative value means "not set", as in the Cubism SDK.
    pub fn fade_in_time(&self) -> Option<f32> {
        self.fade_in_time.filter(|seconds| *seconds >= 0.0)
    }

    /// Returns the fade-out this reference declares, in seconds.
    pub fn fade_out_time(&self) -> Option<f32> {
        self.fade_out_time.filter(|seconds| *seconds >= 0.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
/// Reference to an expression file from `model3.json`.
pub struct ExpressionReference {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "File")]
    file: String,
}

impl ExpressionReference {
    /// Returns the display name for this expression.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the expression file path relative to the model file.
    pub fn file(&self) -> &str {
        &self.file
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
/// A named group of parameter or part ids.
pub struct Group {
    #[serde(rename = "Target")]
    target: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Ids")]
    ids: Vec<String>,
}

impl Group {
    /// Returns the group target, such as `Parameter` or `PartOpacity`.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Returns the Cubism group name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns ids that belong to this group.
    pub fn ids(&self) -> &[String] {
        &self.ids
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
/// A named hit area declared by the model.
pub struct HitArea {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Name")]
    name: String,
}

impl HitArea {
    /// Returns the drawable or part id used for hit testing.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the user-facing hit area name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(motions: &str) -> Model3 {
        let source = format!(
            r#"{{"Version": 3, "FileReferences": {{"Moc": "m.moc3", "Textures": ["t.png"], "Motions": {motions}}}}}"#
        );

        Model3::from_json_str(&source).expect("a valid model file")
    }

    #[test]
    fn a_motion_reference_carries_its_fade_times() {
        let model = model(
            r#"{"Idle": [{"File": "a.motion3.json", "FadeInTime": 0.5, "FadeOutTime": 0.25}]}"#,
        );
        let reference = &model.motions()["Idle"][0];

        assert_eq!(reference.fade_in_time(), Some(0.5));
        assert_eq!(reference.fade_out_time(), Some(0.25));
    }

    #[test]
    fn a_reference_without_fades_says_nothing() {
        let model = model(r#"{"Idle": [{"File": "a.motion3.json"}]}"#);
        let reference = &model.motions()["Idle"][0];

        assert_eq!(reference.fade_in_time(), None);
        assert_eq!(reference.fade_out_time(), None);
    }

    #[test]
    fn a_negative_fade_means_unset() {
        let model = model(r#"{"Idle": [{"File": "a.motion3.json", "FadeInTime": -1.0}]}"#);
        let reference = &model.motions()["Idle"][0];

        assert_eq!(reference.fade_in_time(), None);
    }
}
