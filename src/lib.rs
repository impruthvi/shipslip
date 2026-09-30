//! Shipslip engine.
//!
//! Front-ends (the `slip` CLI, the desktop app) use only this public API.
//! The library never reads stdin or writes to stdout/stderr; it reports
//! progress as [`DeployEvent`]s.
//!
//! A deploy is two-phase:
//!
//! 1. [`prepare`] runs read-only preflight and returns a [`Preview`].
//! 2. [`execute`] runs the deploy, and only accepts a [`Confirmation`]
//!    built from that exact preview.

mod engine;
mod event;
mod lock;
mod preflight;
mod runner;
mod script;
pub mod transport;

pub use engine::{
    break_lock, bring_app_up, cancel, execute, prepare, BreakLockError, BringUpError, ConfirmError,
    Confirmation, DeployTarget, ExecuteError, ExecutionHandle, PrepareError, Preview,
};
pub use event::{DeployEvent, DeployOutcome, MaintenancePhase, StepStatus, StopReason};
pub use lock::{LockInfo, LockOwner, STALE_AFTER};
pub use preflight::{AbortReason, BlockReason};
