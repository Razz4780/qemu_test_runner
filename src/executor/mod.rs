use crate::{ssh::SshAction, Error, Output};
use std::{
    ffi::{OsStr, OsString},
    time::Duration,
};

pub mod base;
pub mod stack;

/// Config for running a sequence of actions on a [crate::qemu::QemuInstance].
pub struct ExecutorConfig {
    /// The user to executing actions.
    pub user: String,
    /// The password for the user.
    pub password: String,
    /// Timeout for opening an SSH connection with the [crate::qemu::QemuInstance] (milliseconds).
    pub connection_timeout: Duration,
    /// Timeout for [crate::qemu::QemuInstance] shutdown (milliseconds).
    pub poweroff_timeout: Duration,
    /// The command that will be used to shutdown the [crate::qemu::QemuInstance].
    pub poweroff_command: String,
}

#[derive(Debug)]
pub struct ActionReport {
    pub action: SshAction,
    pub timeout: Duration,
    pub elapsed_time: Duration,
    pub output: Result<Output, Error>,
}

impl ActionReport {
    fn err(&self) -> Option<&Error> {
        self.output.as_ref().err()
    }
}

/// A report from executing a sequence of actions on a [crate::qemu::QemuInstance].
#[derive(Debug)]
pub struct ExecutorReport {
    /// Path to the image used by the [crate::qemu::QemuInstance].
    image: OsString,
    /// Output of the [crate::qemu::QemuInstance].
    qemu: Result<Output, Error>,
    /// Result of creating the SSH connection.
    connect: Result<(), Error>,
    /// Reports from the executed [SshAction]s.
    action_reports: Vec<ActionReport>,
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

    pub fn action_reports(&self) -> &[ActionReport] {
        &self.action_reports[..]
    }

    pub fn err(&self) -> Option<&Error> {
        self.connect
            .as_ref()
            .err()
            .or_else(|| self.action_reports.last().and_then(ActionReport::err))
            .or_else(|| self.qemu.as_ref().err())
    }
}
