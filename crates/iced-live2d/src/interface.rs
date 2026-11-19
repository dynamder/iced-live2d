//! The vocabulary an interface declares, and the two ways to declare one.

use std::fmt;
use std::hash::Hash;
use std::sync::Arc;

use live2d_render::{AssetId, ModelInfo};

/// How a vocabulary's entries are identified in the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocabularySpec {
    /// Names the host declared, in the order it declared them.
    ///
    /// Each one is looked up in the model when it loads, and a missing name fails
    /// the load: an interface is a promise about the model, not a wish list.
    Static(&'static [&'static str]),
    /// The model's own entries, in model order.
    ///
    /// Nothing is looked up, so nothing can be missing; this is how a model acts
    /// as its own interface.
    Model,
}

/// A vocabulary entry as the render layer identifies it.
///
/// The render layer only knows model indices and names, so this is what comes back
/// out of events; [`Vocabulary::from_id`] turns it into a typed key again.
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

/// One of the five lists an [`Interface`] names.
///
/// Implement it by hand, or derive it with the `macros` feature. The four
/// required items are what the component needs to turn a key into a model index
/// and back; everything else about the key is the host's business.
pub trait Vocabulary: Clone + Eq + Ord + Hash + std::fmt::Debug + Send + Sync + 'static {
    /// How the entries are identified in the model.
    const SPEC: VocabularySpec;

    /// The name the model must declare for this entry.
    fn name(&self) -> &str;

    /// Position in this vocabulary's own sequence.
    fn index(&self) -> usize;

    /// Rebuilds the key the render layer means.
    ///
    /// A derived implementation always succeeds; `None` is for hand-written ones
    /// whose `index` and `from_id` disagree with `SPEC`. The component reports it
    /// as a [`Problem`](crate::Problem) instead of dropping the event.
    fn from_id(id: &KeyId) -> Option<Self>;

    /// The asset a model-scoped key belongs to.
    ///
    /// `None` for a compile-time vocabulary, whose keys mean the same thing on
    /// every asset of the interface.
    fn asset(&self) -> Option<AssetId> {
        None
    }
}

/// The five vocabularies a model is driven through.
///
/// An implementation is five associated types and nothing else — usually declared
/// once next to the enums it names:
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
/// A model that no interface describes fits the same shape through
/// [`Raw`](crate::raw::Raw), which names the model's own entries.
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

/// The five vocabularies of one interface, as plain data.
///
/// This is what crosses thread boundaries instead of the interface itself: the
/// pipeline never sees a host type, only these five specs.
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
    /// Either every name resolves or the whole interface is rejected: a partial
    /// mapping would leave keys that silently address the wrong entry.
    pub(crate) fn resolve(&self, info: &ModelInfo) -> Result<Mapping, MissingNames> {
        let parameters = resolve(self.parameter, info.parameters.len(), |name| {
            info.parameter(name)
        });
        let parts = resolve(self.part, info.parts.len(), |name| info.part(name));
        let motions = resolve(self.motion, info.motions.len(), |name| {
            info.motion(name)
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

/// Which model entries an interface's vocabulary entries point at.
///
/// Indexed by the vocabulary index, which is what a key's `index()` is.
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

    pub(crate) fn motion(&self, index: usize) -> Option<usize> {
        self.motions.get(index).copied()
    }

    pub(crate) fn expression(&self, index: usize) -> Option<usize> {
        self.expressions.get(index).copied()
    }

    /// The vocabulary index of a model motion group.
    ///
    /// Reverse of [`Mapping::motion`], for events that start in the render layer.
    pub(crate) fn motion_vocabulary(&self, group: usize) -> Option<usize> {
        self.motions.iter().position(|mapped| *mapped == group)
    }

    /// The vocabulary index of a model hit area.
    pub(crate) fn hit_area_vocabulary(&self, area: usize) -> Option<usize> {
        self.hit_areas.iter().position(|mapped| *mapped == area)
    }

}

/// The names an interface asked for that a model does not declare.
///
/// An interface that names anything the model lacks fails the load, so this is
/// what a host sees in [`LoadFailure::Interface`](crate::LoadFailure::Interface).
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

/// Resolves one vocabulary, returning the mapping and the names that did not
/// resolve.
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
            bounds: live2d_render::ModelBounds::new([0.0, 0.0], [1.0, 1.0]),
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
        let mapping = SPEC.resolve(&info(&["A", "B"], &["Idle", "Tap"])).expect("resolves");

        assert_eq!(mapping.motion(1), Some(1));
        assert_eq!(mapping.motion_vocabulary(1), Some(1));
    }

    #[test]
    fn the_reverse_mapping_finds_the_vocabulary_index() {
        let mapping = SPEC.resolve(&info(&["A", "B"], &["Tap", "Idle"])).expect("resolves");

        assert_eq!(mapping.motion(0), Some(0));
        assert_eq!(mapping.motion_vocabulary(0), Some(0));
        assert_eq!(mapping.motion_vocabulary(1), Some(1));
        assert_eq!(mapping.motion_vocabulary(2), None);
    }

    #[test]
    fn a_static_vocabulary_of_groups_maps_subset() {
        let spec = InterfaceSpec {
            motion: VocabularySpec::Static(&["Tap"]),
            ..SPEC
        };
        let mapping = spec.resolve(&info(&["A", "B"], &["Idle", "Tap"])).expect("resolves");

        assert_eq!(mapping.motion(0), Some(1));
        assert_eq!(mapping.motion_vocabulary(1), Some(0));
        assert_eq!(mapping.motion_vocabulary(0), None);
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
