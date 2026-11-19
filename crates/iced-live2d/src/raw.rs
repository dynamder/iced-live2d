//! The escape hatch: a model that names its own entries.
//!
//! Clips are indexed as `(group, clip)` wherever they are played, so a motion key
//! names a **group**, not a clip.

use std::marker::PhantomData;
use std::sync::Arc;

use live2d_render::{AssetId, ModelInfo};

use crate::interface::{Interface, KeyId, Vocabulary, VocabularySpec};

/// Marks a key as addressing a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParameterKind;

/// Marks a key as addressing a part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartKind;

/// Marks a key as addressing a motion group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MotionKind;

/// Marks a key as addressing an expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExpressionKind;

/// Marks a key as addressing a hit area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HitAreaKind;

/// A key from a model's own list.
///
/// It carries the asset it was minted from, so handing it to a model of a
/// different asset is rejected instead of silently landing on the same index.
pub struct RawKey<T> {
    asset: AssetId,
    name: Arc<str>,
    index: usize,
    kind: PhantomData<fn() -> T>,
}

impl<T> Clone for RawKey<T> {
    fn clone(&self) -> Self {
        Self {
            asset: self.asset,
            name: Arc::clone(&self.name),
            index: self.index,
            kind: PhantomData,
        }
    }
}

impl<T> PartialEq for RawKey<T> {
    fn eq(&self, other: &Self) -> bool {
        self.asset == other.asset && self.name == other.name && self.index == other.index
    }
}

impl<T> Eq for RawKey<T> {}

impl<T> PartialOrd for RawKey<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for RawKey<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.asset, self.index, &self.name).cmp(&(other.asset, other.index, &other.name))
    }
}

impl<T> std::hash::Hash for RawKey<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.asset.hash(state);
        self.index.hash(state);
        self.name.hash(state);
    }
}

impl<T> std::fmt::Debug for RawKey<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RawKey")
            .field("asset", &self.asset)
            .field("index", &self.index)
            .field("name", &self.name)
            .finish()
    }
}

impl<T> RawKey<T> {
    /// The model's name for this entry.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The model index this key addresses.
    pub fn index(&self) -> usize {
        self.index
    }

    /// The asset this key belongs to.
    pub fn asset(&self) -> AssetId {
        self.asset
    }
}

impl<T: 'static> Vocabulary for RawKey<T> {
    const SPEC: VocabularySpec = VocabularySpec::Model;

    fn name(&self) -> &str {
        self.name()
    }

    fn index(&self) -> usize {
        self.index()
    }

    fn from_id(id: &KeyId) -> Option<Self> {
        Some(Self {
            asset: id.asset,
            name: Arc::clone(&id.name),
            index: id.index,
            kind: PhantomData,
        })
    }

    fn asset(&self) -> Option<AssetId> {
        Some(self.asset)
    }
}

/// A parameter key from the model's own list.
pub type RawParameter = RawKey<ParameterKind>;
/// A part key from the model's own list.
pub type RawPart = RawKey<PartKind>;
/// A motion group key from the model's own list.
pub type RawMotion = RawKey<MotionKind>;
/// An expression key from the model's own list.
pub type RawExpression = RawKey<ExpressionKind>;
/// A hit area key from the model's own list.
pub type RawHitArea = RawKey<HitAreaKind>;

/// The interface that a model declares for itself.
///
/// Every vocabulary is the model's own, so the only way to get a key is to ask a
/// loaded model for one:
///
/// ```text
/// let info = // the Arc<ModelInfo> a ModelLoaded event carried
/// let mouth = raw::parameters(&info)[0].clone();
/// model.parameters().insert(mouth, 1.0);
/// ```
pub struct Raw;

impl Interface for Raw {
    type Parameter = RawParameter;
    type Part = RawPart;
    type Motion = RawMotion;
    type Expression = RawExpression;
    type HitArea = RawHitArea;
}

/// Every parameter the model declares, in model order.
pub fn parameters(info: &ModelInfo) -> Vec<RawParameter> {
    info.parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| RawKey {
            asset: info.asset,
            name: Arc::clone(&parameter.name),
            index,
            kind: PhantomData,
        })
        .collect()
}

/// Every part the model declares, in model order.
pub fn parts(info: &ModelInfo) -> Vec<RawPart> {
    info.parts
        .iter()
        .enumerate()
        .map(|(index, name)| RawKey {
            asset: info.asset,
            name: Arc::clone(name),
            index,
            kind: PhantomData,
        })
        .collect()
}

/// Every motion group the model declares, in model order.
pub fn motions(info: &ModelInfo) -> Vec<RawMotion> {
    info.motions
        .iter()
        .enumerate()
        .map(|(index, group)| RawKey {
            asset: info.asset,
            name: Arc::clone(&group.name),
            index,
            kind: PhantomData,
        })
        .collect()
}

/// Every expression the model declares, in model order.
pub fn expressions(info: &ModelInfo) -> Vec<RawExpression> {
    info.expressions
        .iter()
        .enumerate()
        .map(|(index, name)| RawKey {
            asset: info.asset,
            name: Arc::clone(name),
            index,
            kind: PhantomData,
        })
        .collect()
}

/// Every hit area the model declares, in model order.
pub fn hit_areas(info: &ModelInfo) -> Vec<RawHitArea> {
    info.hit_areas
        .iter()
        .enumerate()
        .map(|(index, area)| RawKey {
            asset: info.asset,
            name: Arc::clone(&area.name),
            index,
            kind: PhantomData,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> ModelInfo {
        ModelInfo {
            asset: AssetId::unique(),
            bounds: live2d_render::ModelBounds::new([0.0, 0.0], [1.0, 1.0]),
            parameters: vec![live2d_render::ParameterRange {
                name: Arc::from("ParamAngleX"),
                minimum: -30.0,
                maximum: 30.0,
                default: 0.0,
            }]
            .into(),
            parts: Arc::from([Arc::from("PartArmA")]),
            motions: vec![live2d_render::MotionGroup {
                name: Arc::from("Idle"),
                clips: 3,
            }]
            .into(),
            expressions: Arc::from([Arc::from("Smile")]),
            hit_areas: vec![live2d_render::HitArea {
                id: Arc::from("HitAreaHead"),
                name: Arc::from("Head"),
            }]
            .into(),
        }
    }

    #[test]
    fn keys_follow_the_model_order() {
        let info = info();
        let parameters = parameters(&info);

        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters[0].index(), 0);
        assert_eq!(parameters[0].name(), "ParamAngleX");
        assert_eq!(parameters[0].asset(), info.asset);
    }

    #[test]
    fn a_key_round_trips_through_the_render_layers_identity() {
        let info = info();
        let key = motions(&info).remove(0);
        let id = KeyId {
            asset: key.asset(),
            index: key.index(),
            name: Arc::from(key.name()),
        };

        assert_eq!(RawMotion::from_id(&id), Some(key));
    }

    #[test]
    fn keys_from_different_assets_are_different() {
        let first = parameters(&info()).remove(0);
        let second = parameters(&info()).remove(0);

        assert_ne!(first, second);
        assert_eq!(first.name(), second.name());
    }

    #[test]
    fn every_kind_is_a_distinct_vocabulary() {
        assert_eq!(RawParameter::SPEC, VocabularySpec::Model);
        assert_eq!(RawPart::SPEC, VocabularySpec::Model);
        assert_eq!(RawMotion::SPEC, VocabularySpec::Model);
        assert_eq!(RawExpression::SPEC, VocabularySpec::Model);
        assert_eq!(RawHitArea::SPEC, VocabularySpec::Model);
    }
}
