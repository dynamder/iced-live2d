//! The vocabulary an interface declares, and the two ways to declare one.

use std::fmt;
use std::hash::Hash;
use std::sync::Arc;

use live2d_render::{AssetId, ModelInfo, MotionGroupIndex};

/// How a vocabulary's entries are identified in the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocabularySpec {
    /// Names the host declared, in the order it declared them.
    ///
    /// A name the model lacks fails the load: an interface is a promise about the
    /// model, not a wish list.
    Static(&'static [&'static str]),
    /// The model's own entries, in model order. Nothing to look up, nothing to
    /// miss — this is how a model acts as its own interface.
    Model,
}

/// A vocabulary entry as the render layer identifies it, which is what events
/// carry. [`Vocabulary::from_id`] turns it back into a typed key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyId {
    /// The asset the entry belongs to.
    pub asset: AssetId,
    /// Position in the vocabulary: the host's order for a [`VocabularySpec::Static`]
    /// vocabulary, the model's own order for a [`VocabularySpec::Model`] one.
    pub index: usize,
    /// The name the entry has in the model.
    pub name: Arc<str>,
}

/// Why a [`Vocabulary`] could not rebuild one of its keys.
///
/// It means the render layer and the vocabulary disagree about an index — an
/// internal mismatch, never a missing name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VocabularyError {
    index: usize,
}

impl VocabularyError {
    /// Reports the index the render layer asked for.
    pub fn new(index: usize) -> Self {
        Self { index }
    }

    /// The index that was asked for.
    pub fn index(self) -> usize {
        self.index
    }
}

impl fmt::Display for VocabularyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "index #{} is not in the vocabulary", self.index)
    }
}

impl std::error::Error for VocabularyError {}

/// One of the five lists an [`Interface`] names: the four items that turn a key
/// into a model index and back. Derive it with the `macros` feature, or write it
/// by hand.
pub trait Vocabulary: Clone + Eq + Ord + Hash + std::fmt::Debug + Send + Sync + 'static {
    /// How the entries are identified in the model.
    const SPEC: VocabularySpec;

    /// The name the model must declare for this entry.
    fn name(&self) -> &str;

    /// Position in this vocabulary's own sequence.
    fn index(&self) -> usize;

    /// Rebuilds the key the render layer means.
    ///
    /// A derived implementation fails only for an index outside the vocabulary. A
    /// hand-written one may also fail when its `index` and `from_id` disagree. The
    /// failure is reported as a [`Problem`](crate::Problem) rather than dropping the
    /// event silently.
    fn from_id(id: &KeyId) -> Result<Self, VocabularyError>;

    /// The asset a model-scoped key belongs to. `None` for a compile-time
    /// vocabulary, whose keys mean the same on every asset.
    fn asset(&self) -> Option<AssetId> {
        None
    }
}

/// The five vocabularies a model is driven through, declared once next to the enums
/// they name:
///
/// ```text
/// impl Interface for Face {
///     type Parameter = FaceParameter;
///     type Part = FacePart;
///     type Motion = FaceMotion;
///     type Expression = FaceExpression;
///     type HitArea = FaceHitArea;
/// }
/// ```
///
/// [`Raw`](crate::raw::Raw) fits the same shape for a model no interface describes.
pub trait Interface: 'static {
    /// Every parameter a host may drive.
    type Parameter: Vocabulary;
    /// Every part a host may hide or reveal.
    type Part: Vocabulary;
    /// Every motion group a host may play; a clip is chosen inside a group.
    type Motion: Vocabulary;
    /// Every expression a host may apply.
    type Expression: Vocabulary;
    /// Every hit area a pointer can land on.
    type HitArea: Vocabulary;
}

/// The five vocabularies of one interface, as plain data: what the render layer
/// sees instead of a host type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceSpec {
    /// The parameter vocabulary.
    pub parameter: VocabularySpec,
    /// The part vocabulary.
    pub part: VocabularySpec,
    /// The motion-group vocabulary.
    pub motion: VocabularySpec,
    /// The expression vocabulary.
    pub expression: VocabularySpec,
    /// The hit-area vocabulary.
    pub hit_area: VocabularySpec,
}

impl InterfaceSpec {
    /// Reads the spec of an interface.
    pub fn of<I: Interface>() -> Self {
        Self {
            parameter: I::Parameter::SPEC,
            part: I::Part::SPEC,
            motion: I::Motion::SPEC,
            expression: I::Expression::SPEC,
            hit_area: I::HitArea::SPEC,
        }
    }

    /// Resolves every vocabulary against one model, once, at load time.
    ///
    /// All of it or none: a partial mapping would leave keys silently addressing
    /// the wrong entry.
    pub(crate) fn resolve(&self, info: &ModelInfo) -> Result<Mapping, MissingNames> {
        let parameters = resolve(self.parameter, info.parameters.len(), |name| {
            info.parameter(name)
        });
        let parts = resolve(self.part, info.parts.len(), |name| info.part(name));
        let motions = resolve(self.motion, info.motions.len(), |name| {
            info.motion(name).map(MotionGroupIndex::get)
        });
        let expressions = resolve(self.expression, info.expressions.len(), |name| {
            info.expression(name)
        });
        let hit_areas = resolve(self.hit_area, info.hit_areas.len(), |name| {
            info.hit_area(name)
        });

        let missing = MissingNames {
            parameters: parameters.1,
            parts: parts.1,
            motions: motions.1,
            expressions: expressions.1,
            hit_areas: hit_areas.1,
        };

        if !missing.is_empty() {
            return Err(missing);
        }

        Ok(Mapping {
            parameters: parameters.0,
            parts: parts.0,
            motions: motions.0,
            expressions: expressions.0,
            hit_areas: hit_areas.0,
        })
    }
}

/// Which model entry each vocabulary index points at.
#[derive(Debug, Clone, Default)]
pub(crate) struct Mapping {
    parameters: Vec<usize>,
    parts: Vec<usize>,
    motions: Vec<usize>,
    expressions: Vec<usize>,
    hit_areas: Vec<usize>,
}

impl Mapping {
    pub(crate) fn parameter(&self, index: usize) -> Option<usize> {
        self.parameters.get(index).copied()
    }

    pub(crate) fn part(&self, index: usize) -> Option<usize> {
        self.parts.get(index).copied()
    }

    pub(crate) fn motion(&self, index: usize) -> Option<MotionGroupIndex> {
        self.motions
            .get(index)
            .map(|group| MotionGroupIndex::new(*group))
    }

    pub(crate) fn expression(&self, index: usize) -> Option<usize> {
        self.expressions.get(index).copied()
    }

    /// The vocabulary index of a model motion group: the reverse of
    /// [`Mapping::motion`], for events that start in the render layer.
    pub(crate) fn motion_vocabulary(&self, group: MotionGroupIndex) -> Option<usize> {
        self.motions
            .iter()
            .position(|mapped| *mapped == group.get())
    }

    /// The vocabulary index of a model hit area.
    pub(crate) fn hit_area_vocabulary(&self, area: usize) -> Option<usize> {
        self.hit_areas.iter().position(|mapped| *mapped == area)
    }
}

/// The names an interface asked for that a model does not declare — what a host
/// sees in [`LoadFailure::Interface`](crate::LoadFailure::Interface).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissingNames {
    /// Parameter names the model does not declare.
    pub parameters: Vec<&'static str>,
    /// Part ids the model does not declare.
    pub parts: Vec<&'static str>,
    /// Motion group names the model does not declare.
    pub motions: Vec<&'static str>,
    /// Expression names the model does not declare.
    pub expressions: Vec<&'static str>,
    /// Hit area names the model does not declare.
    pub hit_areas: Vec<&'static str>,
}

impl MissingNames {
    /// Whether every name resolved.
    pub fn is_empty(&self) -> bool {
        self.parameters.is_empty()
            && self.parts.is_empty()
            && self.motions.is_empty()
            && self.expressions.is_empty()
            && self.hit_areas.is_empty()
    }

    /// How many names are missing, across all five vocabularies.
    pub fn len(&self) -> usize {
        self.parameters.len()
            + self.parts.len()
            + self.motions.len()
            + self.expressions.len()
            + self.hit_areas.len()
    }
}

impl fmt::Display for MissingNames {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let groups = [
            ("parameter", &self.parameters),
            ("part", &self.parts),
            ("motion", &self.motions),
            ("expression", &self.expressions),
            ("hit area", &self.hit_areas),
        ];

        let mut first = true;

        for (kind, names) in groups {
            if names.is_empty() {
                continue;
            }

            if !first {
                formatter.write_str("; ")?;
            }

            first = false;
            write!(formatter, "missing {kind}s {names:?}")?;
        }

        Ok(())
    }
}

/// Resolves one vocabulary: the mapping, and the names that did not resolve.
fn resolve(
    spec: VocabularySpec,
    count: usize,
    look_up: impl Fn(&str) -> Option<usize>,
) -> (Vec<usize>, Vec<&'static str>) {
    match spec {
        VocabularySpec::Model => ((0..count).collect(), Vec::new()),
        VocabularySpec::Static(names) => {
            let mut mapping = Vec::with_capacity(names.len());
            let mut missing = Vec::new();

            for &name in names {
                match look_up(name) {
                    Some(index) => mapping.push(index),
                    None => missing.push(name),
                }
            }

            (mapping, missing)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(parameters: &[&str], motions: &[&str]) -> ModelInfo {
        ModelInfo {
            asset: AssetId::unique(),
            bounds: live2d_render::ModelBounds::try_new([0.0, 0.0], [1.0, 1.0]).expect("a box"),
            parameters: parameters
                .iter()
                .map(|name| live2d_render::ParameterRange {
                    name: Arc::from(*name),
                    minimum: -1.0,
                    maximum: 1.0,
                    default: 0.0,
                })
                .collect(),
            parts: Arc::from([]),
            motions: motions
                .iter()
                .map(|name| live2d_render::MotionGroup {
                    name: Arc::from(*name),
                    clips: 1,
                })
                .collect(),
            expressions: Arc::from([]),
            hit_areas: Arc::from([]),
        }
    }

    const SPEC: InterfaceSpec = InterfaceSpec {
        parameter: VocabularySpec::Static(&["A", "B"]),
        part: VocabularySpec::Model,
        motion: VocabularySpec::Model,
        expression: VocabularySpec::Model,
        hit_area: VocabularySpec::Model,
    };

    #[test]
    fn a_static_vocabulary_maps_by_name() {
        let mapping = SPEC.resolve(&info(&["B", "A"], &[])).expect("resolves");

        assert_eq!(mapping.parameter(0), Some(1));
        assert_eq!(mapping.parameter(1), Some(0));
    }

    #[test]
    fn a_missing_name_rejects_the_whole_interface() {
        let missing = SPEC.resolve(&info(&["A"], &[])).expect_err("is incomplete");

        assert_eq!(missing.parameters, vec!["B"]);
        assert_eq!(missing.len(), 1);
    }

    #[test]
    fn a_model_vocabulary_is_the_identity() {
        let mapping = SPEC
            .resolve(&info(&["A", "B"], &["Idle", "Tap"]))
            .expect("resolves");

        assert_eq!(mapping.motion(1), Some(MotionGroupIndex::new(1)));
        assert_eq!(mapping.motion_vocabulary(MotionGroupIndex::new(1)), Some(1));
    }

    #[test]
    fn the_reverse_mapping_finds_the_vocabulary_index() {
        let mapping = SPEC
            .resolve(&info(&["A", "B"], &["Tap", "Idle"]))
            .expect("resolves");

        assert_eq!(mapping.motion(0), Some(MotionGroupIndex::new(0)));
        assert_eq!(mapping.motion_vocabulary(MotionGroupIndex::new(0)), Some(0));
        assert_eq!(mapping.motion_vocabulary(MotionGroupIndex::new(1)), Some(1));
        assert_eq!(mapping.motion_vocabulary(MotionGroupIndex::new(2)), None);
    }

    #[test]
    fn a_static_vocabulary_of_groups_maps_subset() {
        let spec = InterfaceSpec {
            motion: VocabularySpec::Static(&["Tap"]),
            ..SPEC
        };
        let mapping = spec
            .resolve(&info(&["A", "B"], &["Idle", "Tap"]))
            .expect("resolves");

        assert_eq!(mapping.motion(0), Some(MotionGroupIndex::new(1)));
        assert_eq!(mapping.motion_vocabulary(MotionGroupIndex::new(1)), Some(0));
        assert_eq!(mapping.motion_vocabulary(MotionGroupIndex::new(0)), None);
    }

    #[test]
    fn missing_names_read_as_a_sentence() {
        let missing = MissingNames {
            parameters: vec!["Mouth"],
            motions: vec!["Idle"],
            ..MissingNames::default()
        };

        assert_eq!(
            missing.to_string(),
            "missing parameters [\"Mouth\"]; missing motions [\"Idle\"]"
        );
        assert!(!missing.is_empty());
    }
}
