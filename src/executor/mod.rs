use crate::{ssh::SshAction, Output};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub mod base;
pub mod stack;

/// Config for running an executor.
#[derive(Debug)]
pub struct ExecutorConfig {
    /// The user executing [SshAction]s.
    pub user: String,
    /// The password for the user.
    pub password: String,
    /// Timeout for opening an SSH connection with the [crate::qemu::QemuInstance].
    pub connection_timeout: Duration,
    /// Timeout for [crate::qemu::QemuInstance] shutdown after executing a poweroff command.
    pub poweroff_timeout: Duration,
    /// The command that will be used to shutdown the [crate::qemu::QemuInstance].
    pub poweroff_command: String,
    /// A limit for stdout and stderr of executed commands.
    /// The outputs will be truncated to this length.
    pub output_limit: Option<u64>,
}

/// Report from running an [SshAction].
#[derive(Debug, Serialize)]
pub struct ActionReport {
    action: SshAction,
    timeout_ms: u128,
    elapsed_time_ms: u128,
    output: Output,
}

impl ActionReport {
    /// # Returns
    /// The executed action.
    pub fn action(&self) -> &SshAction {
        &self.action
    }

    /// # Returns
    /// The timeout configured for the executed action (milliseconds).
    pub fn timeout_ms(&self) -> u128 {
        self.timeout_ms
    }

    /// # Returns
    /// Time elapsed while executing the action (milliseconds).
    pub fn elapsed_time_ms(&self) -> u128 {
        self.elapsed_time_ms
    }

    /// # Returns
    /// The result of executing the action.
    pub fn output(&self) -> &Output {
        &self.output
    }

    /// # Returns
    /// Whether the execution was successful.
    pub fn success(&self) -> bool {
        self.output.success()
    }
}

/// A report from running multiple [SshAction]s.
#[derive(Debug, Serialize)]
pub struct ExecutorReport {
    image: PathBuf,
    #[serde(rename(serialize = "ssh_connection_ok"))]
    ssh_ok: bool,
    action_reports: Vec<ActionReport>,
    #[serde(rename(serialize = "qemu_exit_clean"))]
    exit_ok: bool,
}

impl ExecutorReport {
    /// # Returns
    /// Path to the image the actions were executed on.
    pub fn image(&self) -> &Path {
        &self.image
    }

    /// # Returns
    /// Whether the SSH connection was established successfuly.
    pub fn ssh_ok(&self) -> bool {
        self.ssh_ok
    }

    /// # Returns
    /// The reports from the executed actions.
    pub fn action_reports(&self) -> &[ActionReport] {
        &self.action_reports[..]
    }

    /// # Returns
    /// Whether the QEMU process exited successfuly after a shutdown command.
    pub fn exit_ok(&self) -> bool {
        self.exit_ok
    }

    /// # Returns
    /// Whether the execution of all actions was successful.
    pub fn success(&self) -> bool {
        self.ssh_ok && self.action_reports.iter().all(ActionReport::success) && self.exit_ok
    }
}

#[cfg(test)]
impl ExecutorConfig {
    /// # Returns
    /// A simple config for tests.
    /// This config should work with any MINIX3 image,
    /// given that there is a user 'root' with password 'root'.
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
