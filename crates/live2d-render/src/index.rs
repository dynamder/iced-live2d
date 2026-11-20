//! What a position in a model list means.
//!
//! A newtype marks the domain, not the owner: it says "a motion group position",
//! not "a group of this model". Passing one to a model of another asset is still
//! possible and is rejected at the call with an error.

use std::fmt;

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
