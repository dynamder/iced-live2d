//! What a loaded model declares, as plain data a host can read.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::bounds::ModelBounds;
use crate::index::{
    ExpressionIndex, HitAreaIndex, MotionClipIndex, MotionGroupIndex, ParameterIndex, PartIndex,
};

/// Identity of one asset. Keys carry it, so a key handed to a model of another
/// asset is recognisably foreign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(u64);

impl AssetId {
    /// Mints an id no other asset in this process will share.
    pub fn unique() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);

        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// One parameter the model declares.
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterRange {
    /// The model's own spelling, such as `ParamAngleX`.
    pub name: Arc<str>,
    /// Smallest value the model allows.
    pub minimum: f32,
    /// Largest value the model allows.
    pub maximum: f32,
    /// Value the parameter rests at.
    pub default: f32,
}

/// One motion group the model declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotionGroup {
    /// Group name, such as `Idle`. Scanned groups are named by this crate.
    pub name: Arc<str>,
    /// How many clips the group holds; the `clip` half of `(group, clip)`.
    pub clips: usize,
}

/// One hit area the model declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitArea {
    /// The drawable or part a point is tested against.
    pub id: Arc<str>,
    /// The model's user-facing name for the area.
    pub name: Arc<str>,
}

/// Everything a loaded asset declares, as plain data: what a host displays and
/// what the escape-hatch interface enumerates.
///
/// List order is stable within one loaded asset, but it is not the `model3.json`
/// order. Use names for semantics and indices only as identity within that asset.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// The asset this describes.
    pub asset: AssetId,
    /// Model-space rectangle the model was framed from.
    pub bounds: ModelBounds,
    /// Every parameter, with its declared range.
    pub parameters: Arc<[ParameterRange]>,
    /// Every part id.
    pub parts: Arc<[Arc<str>]>,
    /// Every motion group.
    pub motions: Arc<[MotionGroup]>,
    /// Every expression name.
    pub expressions: Arc<[Arc<str>]>,
    /// Every hit area.
    pub hit_areas: Arc<[HitArea]>,
}

impl ModelInfo {
    /// The model index of a parameter, by exact name.
    pub fn parameter(&self, name: &str) -> Option<ParameterIndex> {
        self.parameters
            .iter()
            .position(|item| &*item.name == name)
            .map(ParameterIndex::new)
    }

    /// Turns a raw position into a parameter this model declares.
    pub fn parameter_at(&self, index: usize) -> Option<ParameterIndex> {
        self.parameters
            .get(index)
            .map(|_| ParameterIndex::new(index))
    }

    /// The model index of a part, by exact id.
    pub fn part(&self, name: &str) -> Option<PartIndex> {
        self.parts
            .iter()
            .position(|item| &**item == name)
            .map(PartIndex::new)
    }

    /// Turns a raw position into a part this model declares.
    pub fn part_at(&self, index: usize) -> Option<PartIndex> {
        self.parts.get(index).map(|_| PartIndex::new(index))
    }

    /// The position of a motion group, by exact name.
    pub fn motion(&self, name: &str) -> Option<MotionGroupIndex> {
        self.motions
            .iter()
            .position(|item| &*item.name == name)
            .map(MotionGroupIndex::new)
    }

    /// Turns a raw position into a group this model declares.
    pub fn motion_at(&self, index: usize) -> Option<MotionGroupIndex> {
        self.motions
            .get(index)
            .map(|_| MotionGroupIndex::new(index))
    }

    /// How many clips a motion group holds, or `None` when the model declares no
    /// such group. A declared group can still hold zero clips.
    pub fn motion_clips(&self, group: MotionGroupIndex) -> Option<usize> {
        self.motions.get(group.get()).map(|item| item.clips)
    }

    /// Turns a raw clip position into one the group holds.
    pub fn motion_clip(&self, group: MotionGroupIndex, clip: usize) -> Option<MotionClipIndex> {
        (clip < self.motion_clips(group)?).then(|| MotionClipIndex::new(clip))
    }

    /// The model index of an expression, by declared name.
    pub fn expression(&self, name: &str) -> Option<ExpressionIndex> {
        self.expressions
            .iter()
            .position(|item| &**item == name)
            .map(ExpressionIndex::new)
    }

    /// Turns a raw position into an expression this model declares.
    pub fn expression_at(&self, index: usize) -> Option<ExpressionIndex> {
        self.expressions
            .get(index)
            .map(|_| ExpressionIndex::new(index))
    }

    /// The model index of a hit area, by declared name.
    pub fn hit_area(&self, name: &str) -> Option<HitAreaIndex> {
        self.hit_areas
            .iter()
            .position(|item| &*item.name == name)
            .map(HitAreaIndex::new)
    }

    /// Turns a raw position into a hit area this model declares.
    pub fn hit_area_at(&self, index: usize) -> Option<HitAreaIndex> {
        self.hit_areas.get(index).map(|_| HitAreaIndex::new(index))
    }

    /// The model index of the hit area backed by a drawable id.
    pub fn hit_area_of_drawable(&self, id: &str) -> Option<HitAreaIndex> {
        self.hit_areas
            .iter()
            .position(|item| &*item.id == id)
            .map(HitAreaIndex::new)
    }

    /// Every motion file the model ships with, across all groups.
    pub fn motion_count(&self) -> usize {
        self.motions.iter().map(|group| group.clips).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(clips: &[usize]) -> ModelInfo {
        ModelInfo {
            asset: AssetId::unique(),
            bounds: ModelBounds::try_new([0.0, 0.0], [1.0, 1.0]).expect("a box"),
            parameters: Arc::from([]),
            parts: Arc::from([]),
            motions: clips
                .iter()
                .map(|&clips| MotionGroup {
                    name: Arc::from("group"),
                    clips,
                })
                .collect(),
            expressions: Arc::from([]),
            hit_areas: Arc::from([]),
        }
    }

    #[test]
    fn an_unknown_group_has_no_clip_count() {
        assert_eq!(info(&[1, 2]).motion_clips(MotionGroupIndex::new(2)), None);
    }

    #[test]
    fn a_declared_group_reports_its_clips_even_at_zero() {
        assert_eq!(
            info(&[0, 2]).motion_clips(MotionGroupIndex::new(0)),
            Some(0)
        );
    }

    #[test]
    fn a_group_position_is_typed_only_inside_the_list() {
        let info = info(&[1, 2]);

        assert_eq!(info.motion_at(1), Some(MotionGroupIndex::new(1)));
        assert_eq!(info.motion_at(2), None);
    }

    #[test]
    fn a_clip_position_is_typed_only_inside_its_group() {
        let info = info(&[2, 0]);

        assert_eq!(
            info.motion_clip(MotionGroupIndex::new(0), 1),
            Some(MotionClipIndex::new(1))
        );
        assert_eq!(info.motion_clip(MotionGroupIndex::new(0), 2), None);
        assert_eq!(info.motion_clip(MotionGroupIndex::new(1), 0), None);
    }

    fn listed() -> ModelInfo {
        ModelInfo {
            parameters: vec![ParameterRange {
                name: Arc::from("ParamAngleX"),
                minimum: -30.0,
                maximum: 30.0,
                default: 0.0,
            }]
            .into(),
            parts: Arc::from([Arc::from("PartArmA")]),
            expressions: Arc::from([Arc::from("Smile")]),
            hit_areas: vec![HitArea {
                id: Arc::from("ArtMesh1"),
                name: Arc::from("Head"),
            }]
            .into(),
            ..info(&[])
        }
    }

    #[test]
    fn every_list_types_its_own_positions() {
        let info = listed();

        assert_eq!(info.parameter("ParamAngleX"), Some(ParameterIndex::new(0)));
        assert_eq!(info.part("PartArmA"), Some(PartIndex::new(0)));
        assert_eq!(info.expression("Smile"), Some(ExpressionIndex::new(0)));
        assert_eq!(info.hit_area("Head"), Some(HitAreaIndex::new(0)));
        assert_eq!(
            info.hit_area_of_drawable("ArtMesh1"),
            Some(HitAreaIndex::new(0))
        );
    }

    #[test]
    fn a_name_the_model_lacks_has_no_position() {
        let info = listed();

        assert_eq!(info.parameter("AngleX"), None);
        assert_eq!(info.part("PartArmB"), None);
        assert_eq!(info.expression("Frown"), None);
        assert_eq!(info.hit_area("Body"), None);
    }

    #[test]
    fn a_raw_position_is_typed_only_inside_its_list() {
        let info = listed();

        assert_eq!(info.parameter_at(0), Some(ParameterIndex::new(0)));
        assert_eq!(info.parameter_at(1), None);
        assert_eq!(info.part_at(0), Some(PartIndex::new(0)));
        assert_eq!(info.part_at(1), None);
        assert_eq!(info.expression_at(0), Some(ExpressionIndex::new(0)));
        assert_eq!(info.expression_at(1), None);
        assert_eq!(info.hit_area_at(0), Some(HitAreaIndex::new(0)));
        assert_eq!(info.hit_area_at(1), None);
    }
}
