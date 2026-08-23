//! Evidence-based observation of installation completion.
//!
//! Each submodule owns one kind of evidence. This file holds only what all of
//! them share, so a new observation route adds a submodule and nothing else.

pub mod packaged;
pub mod timeout;
pub mod win32;

pub use packaged::*;
pub use timeout::*;
pub use win32::*;

/// UTC millisecond timestamp supplied by the platform adapter.
///
/// Core compares it only with timestamps from the same platform observation
/// flow; it does not derive wall-clock time on its own.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ObservationTimestamp(u64);

impl ObservationTimestamp {
    /// Construct a timestamp supplied by a platform adapter.
    #[must_use]
    pub const fn from_unix_millis(value: u64) -> Self {
        Self(value)
    }

    /// Return the adapter-provided UTC millisecond count.
    #[must_use]
    pub const fn unix_millis(self) -> u64 {
        self.0
    }
}
