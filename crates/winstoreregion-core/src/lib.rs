#![forbid(unsafe_code)]

//! Shared domain types and rules for `WinStoreRegion`.
//!
//! Every module owns one subject and states it in its own `//!` header. This
//! file is the crate facade: it holds product identity, declares the modules,
//! and re-exports their public items so callers keep one flat import path and
//! never depend on where an item currently lives.
//!
//! Placement rule for new code, in the same order:
//!
//! 1. A rule about one subject belongs in that subject's module.
//! 2. A rule that joins two subjects belongs in the module that owns the
//!    decision, not in whichever module is easier to reach.
//! 3. A value shared by several modules with no rules of its own belongs in the
//!    smallest module that can own it, never in this file.
//!
//! This file grows only for product identity. Anything with behaviour, a
//! failure mode, or a test belongs in a module.

pub mod availability;
pub mod diagnostic;
pub mod input;
pub mod install;
pub mod journal;
pub mod launch;
pub mod log;
pub mod machine;
pub mod observe;
mod operation;
pub mod prerequisite;
pub mod recovery;
pub mod region;
pub mod resolve;
pub mod source;
pub mod store_page;
pub mod time;

#[cfg(test)]
mod test_support;

pub use availability::*;
pub use diagnostic::*;
pub use input::*;
pub use install::*;
pub use journal::*;
pub use launch::*;
pub use log::*;
pub use machine::*;
pub use observe::*;
pub use operation::*;
pub use prerequisite::*;
pub use recovery::*;
pub use region::*;
pub use resolve::*;
pub use source::*;
pub use store_page::*;
pub use time::*;

/// Display name used by the GUI and version resource.
pub const APPLICATION_NAME: &str = "WinStoreRegion";

/// Name of the single executable distributed to users.
pub const EXECUTABLE_FILE_NAME: &str = "WinStoreRegion.exe";

/// Per-user directory name below `%LOCALAPPDATA%`.
pub const LOCAL_DATA_DIRECTORY: &str = "WinStoreRegion";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_identity_uses_one_product_name() {
        assert_eq!(APPLICATION_NAME, LOCAL_DATA_DIRECTORY);
        assert_eq!(EXECUTABLE_FILE_NAME, format!("{APPLICATION_NAME}.exe"));
    }
}
