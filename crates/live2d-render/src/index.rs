//! What a position in a model list means.
//!
//! A newtype marks the domain, not the owner: it says "a parameter position", not
//! "a parameter of this model". Passing one to a model of another asset is still
//! possible; what the type prevents is passing a part position where a parameter
//! position belongs.

use std::fmt;

/// A parameter's position in [`ModelInfo::parameters`](crate::ModelInfo::parameters).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParameterIndex(usize);

impl ParameterIndex {
    /// Names `index` as a parameter position.
    pub fn new(index: usize) -> Self {
        Self(index)
    }

    /// The position itself.
    pub fn get(self) -> usize {
        self.0
    }
}

impl fmt::Display for ParameterIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A part's position in [`ModelInfo::parts`](crate::ModelInfo::parts).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartIndex(usize);

impl PartIndex {
    /// Names `index` as a part position.
    pub fn new(index: usize) -> Self {
        Self(index)
    }

    /// The position itself.
    pub fn get(self) -> usize {
        self.0
    }
}

impl fmt::Display for PartIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// An expression's position in
/// [`ModelInfo::expressions`](crate::ModelInfo::expressions).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExpressionIndex(usize);

impl ExpressionIndex {
    /// Names `index` as an expression position.
    pub fn new(index: usize) -> Self {
        Self(index)
    }

    /// The position itself.
    pub fn get(self) -> usize {
        self.0
    }
}

impl fmt::Display for ExpressionIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A hit area's position in [`ModelInfo::hit_areas`](crate::ModelInfo::hit_areas).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HitAreaIndex(usize);

impl HitAreaIndex {
    /// Names `index` as a hit area position.
    pub fn new(index: usize) -> Self {
        Self(index)
    }

    /// The position itself.
    pub fn get(self) -> usize {
        self.0
    }
}

impl fmt::Display for HitAreaIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A motion group's position in [`ModelInfo::motions`](crate::ModelInfo::motions).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MotionGroupIndex(usize);

impl MotionGroupIndex {
    /// Names `index` as a motion group position.
    pub fn new(index: usize) -> Self {
        Self(index)
    }

    /// The position itself.
    pub fn get(self) -> usize {
        self.0
    }
}

impl fmt::Display for MotionGroupIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A clip's position inside one motion group.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MotionClipIndex(usize);

impl MotionClipIndex {
    /// Names `index` as a clip position.
    pub fn new(index: usize) -> Self {
        Self(index)
    }

    /// The position itself.
    pub fn get(self) -> usize {
        self.0
    }
}

impl fmt::Display for MotionClipIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}
