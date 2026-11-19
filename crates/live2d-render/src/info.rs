//! What a loaded model declares, as plain data a host can read.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::bounds::ModelBounds;

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
/// Lists are in model order, which is the order a model-scoped vocabulary indexes
/// into.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// The asset this describes.
    pub asset: AssetId,
    /// Model-space rectangle the model was framed from.
    pub bounds: ModelBounds,
    /// Every parameter, in model order, with its declared range.
    pub parameters: Arc<[ParameterRange]>,
    /// Every part id, in model order.
    pub parts: Arc<[Arc<str>]>,
    /// Every motion group, in model order.
    pub motions: Arc<[MotionGroup]>,
    /// Every expression name, in model order.
    pub expressions: Arc<[Arc<str>]>,
    /// Every hit area, in model order.
    pub hit_areas: Arc<[HitArea]>,
}

impl ModelInfo {
    /// The model index of a parameter, by exact name.
    pub fn parameter(&self, name: &str) -> Option<usize> {
        self.parameters.iter().position(|item| &*item.name == name)
    }

    /// The model index of a part, by exact id.
    pub fn part(&self, name: &str) -> Option<usize> {
        self.parts.iter().position(|item| &**item == name)
    }

    /// The model index of a motion group, by exact name.
    pub fn motion(&self, name: &str) -> Option<usize> {
        self.motions.iter().position(|item| &*item.name == name)
    }

    /// How many clips a motion group holds. Out-of-range groups hold none.
    pub fn motion_clips(&self, group: usize) -> usize {
        self.motions.get(group).map_or(0, |item| item.clips)
    }

    /// The model index of an expression, by declared name.
    pub fn expression(&self, name: &str) -> Option<usize> {
        self.expressions.iter().position(|item| &**item == name)
    }

    /// The model index of a hit area, by declared name.
    pub fn hit_area(&self, name: &str) -> Option<usize> {
        self.hit_areas.iter().position(|item| &*item.name == name)
    }

    /// The model index of the hit area backed by a drawable id.
    pub fn hit_area_of_drawable(&self, id: &str) -> Option<usize> {
        self.hit_areas.iter().position(|item| &*item.id == id)
    }

    /// Every motion file the model ships with, across all groups.
    pub fn motion_count(&self) -> usize {
        self.motions.iter().map(|group| group.clips).sum()
    }
}
