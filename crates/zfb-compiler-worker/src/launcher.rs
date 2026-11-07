use crate::protocol::TEST_HOOKS_ENV;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::Command;

/// The private argument that puts the zfb CLI into compiler-worker mode.
pub const PRIVATE_ARG: &str = "__zfb-compiler-worker";

/// How to start a compiler worker process: a host executable that calls
/// [`crate::run_if_requested`] or [`crate::serve_stdio`] for these arguments.
///
/// Libraries never derive this from `current_exe()`: only the host knows
/// whether its executable understands a private worker mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLauncher {
    program: PathBuf,
    args: Vec<OsString>,
    env: Vec<(OsString, Option<OsString>)>,
}

impl WorkerLauncher {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// The zfb CLI launcher: this executable plus [`PRIVATE_ARG`]. Only the
    /// zfb binary, whose `main` dispatches that argument, may use it.
    pub fn zfb_cli() -> std::io::Result<Self> {
        Ok(Self::new(std::env::current_exe()?)
            .arg(PRIVATE_ARG)
            .env_remove(TEST_HOOKS_ENV))
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), Some(value.into())));
        self
    }

    pub fn env_remove(mut self, key: impl Into<OsString>) -> Self {
        self.env.push((key.into(), None));
        self
    }

    pub fn program(&self) -> &std::path::Path {
        &self.program
    }

    pub fn args(&self) -> impl Iterator<Item = &OsStr> {
        self.args.iter().map(OsString::as_os_str)
    }

    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        for (key, value) in &self.env {
            match value {
                Some(value) => command.env(key, value),
                None => command.env_remove(key),
            };
        }
        command
    }
}
