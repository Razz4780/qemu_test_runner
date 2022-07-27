use crate::{qemu::QemuInstance, ssh::SshHandle, Output, Result, Timeout};
use std::{cmp, io, path::PathBuf, thread, time::Duration};

/// An action that can be executed on a running [QemuInstance].
#[derive(Debug, Clone)]
pub enum Action {
    /// Transfer a file through SSH.
    Send {
        /// Local path to the source file.
        local: PathBuf,
        /// Remote path to the destination.
        remote: PathBuf,
        /// UNIX permissions of the destination file.
        mode: i32,
        /// File transfer timeout.
        timeout: Duration,
    },
    /// Execute a command through SSH.
    Exec {
        /// Command to be executed.
        cmd: String,
        /// Command timeout.
        timeout: Duration,
    },
}

/// Config for running a sequence of [Action]s on a [QemuInstance].
pub struct ExecutionConfig {
    /// The user these [Action]s will be executed by.
    pub user: String,
    /// The password for the user.
    pub password: String,
    /// [Action]s to be executed.
    pub actions: Vec<Action>,
    /// Timeout for [QemuInstance] startup.
    pub startup_timeout: Duration,
    /// Timeout for [QemuInstance] shutdown.
    pub poweroff_timeout: Duration,
    /// The command that will be used to shutdown the [QemuInstance].
    pub poweroff_command: String,
}

/// A report from executing a sequence of [Action]s on a [QemuInstance].
#[derive(Debug)]
pub struct ExecutionReport {
    /// Result of spawning the [QemuInstance].
    pub qemu: Result<Output>,
    /// Result of creating the SSH connection.
    pub connect: Result<()>,
    /// Results of the executed [Action]s.
    pub actions: Vec<Result<Output>>,
    /// Result of the shutdown command.
    pub poweroff: Option<Result<Output>>,
}

/// A wrapper over a [QemuInstance].
/// Used to run an [ExecutionConfig].
pub struct Executor {
    qemu: Option<QemuInstance>,
}

impl Executor {
    /// Creates a new instance of this struct.
    /// This instance will operate on the given [QemuInstance].
    pub fn new(qemu: QemuInstance) -> Self {
        Self { qemu: Some(qemu) }
    }

    /// Creates a new [SshHandle] for the inner [QemuInstance].
    async fn get_ssh_handle(&self, config: &ExecutionConfig) -> io::Result<SshHandle> {
        let ssh_addr = self.qemu.as_ref().unwrap().ssh();
        SshHandle::new(
            ssh_addr,
            config.user.clone(),
            config.password.clone(),
            config.startup_timeout,
        )
        .await
    }

    /// Kills the inner [QemuInstance] and waits for its [Output].
    async fn kill_qemu(mut self) -> Result<Output> {
        let mut qemu = self.qemu.take().unwrap();
        qemu.kill().await.ok();
        qemu.wait().await
    }

    /// Waits for the [Output] of the inner [QemuInstance].
    async fn wait_qemu(mut self, timeout: Duration) -> Result<Output> {
        let timeout = Timeout::new(timeout);

        loop {
            let exited = self.qemu.as_mut().unwrap().try_wait()?.is_some();
            if exited {
                break self.qemu.take().unwrap().wait().await;
            }
            match timeout.remaining() {
                Ok(remaining) => thread::sleep(cmp::min(remaining, Duration::from_secs(1))),
                Err(_) => break self.kill_qemu().await,
            }
        }
    }

    /// Runs the given [ExecutionConfig] on the inner [QemuInstance].
    /// The instance is shut down after the last [Action] in the config.
    /// The [Action]s are executed as long as there is no error.
    pub async fn run(self, config: &ExecutionConfig) -> ExecutionReport {
        let mut ssh = match self.get_ssh_handle(config).await {
            Ok(ssh) => ssh,
            Err(e) => {
                return ExecutionReport {
                    qemu: self.kill_qemu().await,
                    connect: Err(e.into()),
                    actions: Default::default(),
                    poweroff: None,
                }
            }
        };

        let mut results = Vec::with_capacity(config.actions.len());
        for action in &config.actions {
            let res = match action {
                Action::Exec { cmd, timeout } => ssh.exec(cmd.clone(), *timeout).await,
                Action::Send {
                    local,
                    remote,
                    mode,
                    timeout,
                } => ssh
                    .send(local.clone(), remote.clone(), *mode, *timeout)
                    .await
                    .map(|_| Output {
                        stdout: Default::default(),
                        stderr: Default::default(),
                    }),
            };

            results.push(res);
            if results.last().unwrap().is_err() {
                break;
            }
        }

        let kill = results.last().map(Result::is_err).unwrap_or(false);
        let (qemu, poweroff) = if kill {
            (self.kill_qemu().await, None)
        } else {
            let poweroff = ssh
                .exec(config.poweroff_command.clone(), config.poweroff_timeout)
                .await;
            (
                self.wait_qemu(config.poweroff_timeout).await,
                Some(poweroff),
            )
        };

        ExecutionReport {
            qemu,
            connect: Ok(()),
            actions: results,
            poweroff,
        }
    }
}
