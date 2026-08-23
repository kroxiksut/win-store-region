//! The recovery record and every decision that depends on it.
//!
//! `record` owns the durable shape, `store` owns publication, and `startup`
//! owns the conflict-safe choices. Splitting them keeps the recovery invariant
//! readable: nothing here may clear a record that was not verified first.

pub mod record;
pub mod startup;
pub mod store;

pub use record::*;
pub use startup::*;
pub use store::*;
