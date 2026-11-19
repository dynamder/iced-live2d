//! How long a motion fades when nothing in the model folder says.

use std::time::Duration;

use mocari::motion::DEFAULT_FADE_SECONDS;

/// How long a motion takes to fade when neither its file nor the model file says.
///
/// [`Fade::default`] is Cubism's own one second, so a host that never touches this
/// matches the Cubism Viewer. A motion that declares `FadeInTime` / `FadeOutTime`,
/// or whose `model3.json` reference does, keeps that value: the files are the
/// artist's decision and this is only what fills the gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fade(Duration);

impl Default for Fade {
    fn default() -> Self {
        Self(Duration::from_secs_f32(DEFAULT_FADE_SECONDS))
    }
}

impl Fade {
    /// No fade at all, which is what a host that wants a cut sets.
    pub const fn zero() -> Self {
        Self(Duration::ZERO)
    }

    /// The same fade in seconds, the unit the player and the files speak.
    pub fn seconds(self) -> f32 {
        self.0.as_secs_f32()
    }

    /// Whether this fade does nothing.
    pub fn is_zero(self) -> bool {
        self.0.is_zero()
    }
}

impl From<Duration> for Fade {
    fn from(duration: Duration) -> Self {
        Self(duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_sdk_second() {
        assert_eq!(Fade::default().seconds(), DEFAULT_FADE_SECONDS);
        assert!(!Fade::default().is_zero());
    }

    #[test]
    fn zero_does_nothing() {
        assert!(Fade::zero().is_zero());
        assert_eq!(Fade::zero().seconds(), 0.0);
    }

    #[test]
    fn a_duration_round_trips() {
        let fade = Fade::from(Duration::from_millis(250));

        assert_eq!(fade.seconds(), 0.25);
    }
}
