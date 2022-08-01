use crate::{ssh::SshCommand, DurationMs, Error, Output};
use serde::Deserialize;
use std::{
    ffi::{OsStr, OsString},
    sync::Arc,
    time::Duration,
};

pub mod base;
pub mod stacking;

/// Config for running a sequence of actions on a [crate::qemu::QemuInstance].
#[derive(Deserialize)]
pub struct Config {
    /// The user to executing actions.
    pub user: String,
    /// The password for the user.
    pub password: String,
    /// Timeout for opening an SSH connection with the [crate::qemu::QemuInstance] (milliseconds).
    pub connection_timeout: DurationMs,
    /// Timeout for [crate::qemu::QemuInstance] shutdown (milliseconds).
    pub poweroff_timeout: DurationMs,
    /// The command that will be used to shutdown the [crate::qemu::QemuInstance].
    pub poweroff_command: String,
}

#[derive(Debug)]
pub struct StepReport {
    pub cmd: Arc<SshCommand>,
    pub timeout: Duration,
    pub elapsed_time: Duration,
    pub output: Result<Output, Error>,
}

impl StepReport {
    fn ok(&self) -> Result<(), &Error> {
        self.output.as_ref().map(|_| ())
    }
}

/// A report from executing a sequence of actions on a [QemuInstance].
#[derive(Debug)]
pub struct ExecutorReport {
    /// Path to the image used by the [QemuInstance].
    image: OsString,
    /// Output of the [QemuInstance].
    qemu: Result<Output, Error>,
    /// Result of creating the SSH connection.
    connect: Result<(), Error>,
    /// Reports from the executed steps.
    steps: Vec<StepReport>,
}

impl ExecutorReport {
    pub fn image(&self) -> &OsStr {
        &self.image
    }

    pub fn qemu(&self) -> Result<&Output, &Error> {
        self.qemu.as_ref()
    }

    pub fn connect(&self) -> Option<&Error> {
        self.connect.as_ref().err()
    }

    pub fn steps(&self) -> &[StepReport] {
        &self.steps[..]
    }

    pub fn ok(&self) -> Result<(), &Error> {
        self.connect.as_ref()?;
        self.steps.last().map(StepReport::ok).transpose()?;
        self.qemu.as_ref().map(|_| ())
    }
}
