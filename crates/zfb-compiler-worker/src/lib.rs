//! A supervised native compiler worker (#4630).
//!
//! The parent spawns one worker process per job through an explicit
//! [`WorkerLauncher`], sends one typed [`BundleRequest`] over stdin and reads
//! one typed result from stdout. The worker writes bytes only into the private
//! directory the parent chose: the request reaches it unchanged, so graph
//! keys still name the caller's destinations, and each generated file lands at
//! `<private>/files/<n>` with the destination, size and SHA-256 it belongs to.
//! The parent owns the deadline, the worker
//! identity, publication permission and cleanup: it publishes to the caller's
//! destinations only after the worker exited successfully and every reported
//! file validated.
//!
//! A host executable enters worker mode by calling [`run_if_requested`] (or
//! [`serve_stdio`]) before initializing anything else. The zfb CLI does this
//! for [`PRIVATE_ARG`]; embedders provide their own host and launcher.
mod launcher;
mod parent;
mod protocol;
mod worker;

pub use launcher::{WorkerLauncher, PRIVATE_ARG};
pub use parent::{
    CancelHandle, CompileJob, CompileOutput, CompilerWorker, PublishedFile, RunningJob,
    WorkerError, WorkerIdentity,
};
#[doc(hidden)]
pub use protocol::{TestHook, TEST_HOOKS_ENV};
pub use worker::{run_if_requested, serve_stdio};
pub use zfb_rolldown_prototype::request;
pub use zfb_rolldown_prototype::BundleRequest;
