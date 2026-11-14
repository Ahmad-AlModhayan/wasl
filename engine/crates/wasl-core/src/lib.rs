//! Sync planner, model and safety guards. Pure: no I/O, no clock, no randomness (SRS FR-E2).

#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();

pub mod conditions;
pub mod docs;
pub mod guards;
pub mod identity;
pub mod model;
pub mod names;
pub mod nway;
pub mod paths;
pub mod planner;
pub mod run;
pub mod vv;

pub use model::*;
pub use planner::{PlanOutcome, Planner, PlannerConfig};
pub use run::{ErrorKind, RunStatus};
