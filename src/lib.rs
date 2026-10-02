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

pub mod config;
mod engine;
mod event;
mod lock;
pub mod logs;
pub mod observation;
mod preflight;
mod private_file;
pub mod receipt;
mod runner;
mod script;
pub mod transport;

pub use engine::{
    attach, break_lock, bring_app_up, cancel, execute, execute_recorded, lock_status, prepare,
    prepare_with_plan, AttachError, BreakLockError, BringUpError, ConfirmError, Confirmation,
    DeployTarget, ExecuteError, ExecuteRejected, ExecutionHandle, LockStatusError, LogChannel,
    PrepareError, Preview, RunPlan, POST_DEPLOY_WATCH,
};
pub use event::{DeployEvent, DeployOutcome, MaintenancePhase, StepStatus, StopReason};
pub use lock::{LockInfo, LockOwner, STALE_AFTER};
pub use observation::{ErrorGroup, ErrorVariant, LogPhase, SmokeResult, WatchResult, WatchStatus};
pub use preflight::{AbortReason, BlockReason};
