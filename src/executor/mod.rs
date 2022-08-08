use crate::{ssh::SshAction, Output};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub mod base;
pub mod stack;

/// Config for running a sequence of actions on a [crate::qemu::QemuInstance].
#[derive(Debug)]
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
    /// A limit for stdout and stderr of executed commands.
    pub output_limit: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ActionReport {
    action: SshAction,
    timeout_ms: u128,
    elapsed_time_ms: u128,
    output: Output,
}

impl ActionReport {
    fn success(&self) -> bool {
        self.output.success()
    }
}

/// A report from executing a sequence of actions on a [crate::qemu::QemuInstance].
#[derive(Debug, Serialize)]
pub struct ExecutorReport {
    /// Path to the image used by the [crate::qemu::QemuInstance].
    image: PathBuf,
    /// Whether the SSH connection was established before timeout.
    ssh_ok: bool,
    /// Reports from the executed [SshAction]s.
    action_reports: Vec<ActionReport>,
    /// Wheter the QEMU process exited with success after a poweroff command.
    exit_ok: bool,
}

impl ExecutorReport {
    pub fn image(&self) -> &Path {
        &self.image
    }

    pub fn ssh_ok(&self) -> bool {
        self.ssh_ok
    }

    pub fn action_reports(&self) -> &[ActionReport] {
        &self.action_reports[..]
    }

    pub fn exit_ok(&self) -> bool {
        self.exit_ok
    }

    pub fn success(&self) -> bool {
        self.ssh_ok && self.action_reports.iter().all(ActionReport::success) && self.exit_ok
    }
}

#[cfg(test)]
impl ExecutorConfig {
    pub fn test() -> Self {
        Self {
            user: "root".into(),
            password: "root".into(),
            connection_timeout: Duration::from_secs(20),
            poweroff_timeout: Duration::from_secs(20),
            poweroff_command: "/sbin/poweroff".into(),
            output_limit: None,
        }
    }
}
