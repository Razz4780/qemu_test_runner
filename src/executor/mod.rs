use crate::{ssh::SshCommand, DurationMs, Output, Result};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};

pub mod base;

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
    /// Timeout for blocking libssh2 calls (milliseconds).
    pub blocking_ssh_calls_timeout: DurationMs,
}

#[derive(Debug)]
pub struct StepReport {
    pub cmd: Arc<SshCommand>,
    pub timeout: Duration,
    pub elapsed_time: Duration,
    pub output: Result<Output>,
}

impl StepReport {
    fn success(&self) -> bool {
        self.output.is_ok()
    }
}
