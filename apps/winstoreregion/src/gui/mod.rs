//! Native Win32 presentation layer.
//!
//! The GUI owns the message loop, controls, and dialogs. It renders state and
//! dispatches user actions; it never duplicates a rule that belongs to core.

pub(crate) mod command;
pub(crate) mod controls;
pub(crate) mod diagnostic;
pub(crate) mod dialogs;
pub(crate) mod direction;
pub(crate) mod dragdrop;
mod handoff;
pub(crate) mod ids;
mod install;
mod install_trace;
pub(crate) mod journal;
pub(crate) mod layout;
pub(crate) mod menu;
pub(crate) mod recovery;
pub(crate) mod render;
pub(crate) mod state;
pub(crate) mod strings;
pub(crate) mod window;
pub(crate) mod work;

/// The single entry point of the presentation layer.
pub(crate) use window::run;
