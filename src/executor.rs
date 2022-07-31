use crate::{qemu::QemuInstance, ssh::SshHandle, Output};
use std::{
    ffi::{OsStr, OsString},
    io,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::time::{self, error::Elapsed};

#[derive(Debug)]
pub struct StepReport {
    pub description: String,
    pub elapsed_time: Duration,
    pub output: crate::Result<Output>,
}

impl StepReport {
    fn success(&self) -> bool {
        self.output.is_ok()
    }
}

/// Config for running a sequence of actions on a [QemuInstance].
pub struct ExecutionConfig {
    /// The user to executing actions.
    pub user: String,
    /// The password for the user.
    pub password: String,
    /// Timeout for opening an SSH connection with the [QemuInstance].
    pub connection_timeout: Duration,
    /// Timeout for [QemuInstance] shutdown.
    pub poweroff_timeout: Duration,
    /// The command that will be used to shutdown the [QemuInstance].
    pub poweroff_command: String,
    /// Timeout for blocking libssh2 calls.
    pub blocking_ssh_calls_timeout: Duration,
}

/// A report from executing a sequence of actions on a [QemuInstance].
#[derive(Debug)]
pub struct ExecutorReport {
    /// Path to the image used by the [QemuInstance].
    image: OsString,
    /// Output of the [QemuInstance].
    qemu: crate::Result<Output>,
    /// Result of creating the SSH connection.
    connect: io::Result<()>,
    /// Reports from the executed steps.
    steps: Vec<StepReport>,
}

impl ExecutorReport {
    pub fn image(&self) -> &OsStr {
        &self.image
    }

    pub fn qemu(&self) -> Result<&Output, &crate::Error> {
        self.qemu.as_ref()
    }

    pub fn connect(&self) -> Option<&io::Error> {
        self.connect.as_ref().err()
    }

    pub fn steps(&self) -> &[StepReport] {
        &self.steps[..]
    }

    pub fn success(&self) -> bool {
        self.qemu.is_ok() && self.connect.is_ok() && self.steps.iter().all(StepReport::success)
    }
}

/// A wrapper over a [QemuInstance].
/// Used to interact with the instance over SSH.
pub struct Executor {
    qemu: QemuInstance,
    config: ExecutionConfig,
    ssh: io::Result<SshHandle>,
    step_reports: Vec<StepReport>,
}

impl Executor {
    /// Creates a new instance of this struct.
    /// This instance will operate on the given [QemuInstance].
    pub async fn new(qemu: QemuInstance, config: ExecutionConfig) -> Self {
        let ssh = time::timeout(config.connection_timeout, async {
            loop {
                let handle = match qemu.ssh().await {
                    Ok(addr) => {
                        SshHandle::new(
                            addr,
                            config.user.clone(),
                            config.password.clone(),
                            config.blocking_ssh_calls_timeout,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };

                if let Ok(handle) = handle {
                    return handle;
                }

                time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| io::ErrorKind::TimedOut.into());

        Self {
            qemu,
            config,
            ssh,
            step_reports: Default::default(),
        }
    }

    pub async fn execute(&mut self, command: String, timeout: Duration) -> bool {
        match self.ssh.as_mut() {
            Ok(ssh) => {
                let description = format!("execute command '{}'", command);

                let start = Instant::now();
                let res = time::timeout(timeout, ssh.exec(command)).await;
                let elapsed_time = start.elapsed();

                let res = match res {
                    Ok(Ok(output)) => Ok(output),
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(error.into()),
                };
                let is_ok = res.is_err();

                self.step_reports.push(StepReport {
                    description,
                    elapsed_time,
                    output: res,
                });

                is_ok
            }
            Err(_) => false,
        }
    }

    pub async fn send(
        &mut self,
        local: PathBuf,
        remote: PathBuf,
        mode: i32,
        timeout: Duration,
    ) -> bool {
        match self.ssh.as_mut() {
            Ok(ssh) => {
                let description = format!(
                    "send file '{}' to '{}', mode 0o{:o}",
                    local.display(),
                    remote.display(),
                    mode
                );

                let start = Instant::now();
                let res = time::timeout(timeout, ssh.send(local, remote, mode)).await;
                let elapsed_time = start.elapsed();

                let res = match res {
                    Ok(Ok(_)) => Ok(Output::default()),
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(error.into()),
                };
                let is_ok = res.is_err();

                self.step_reports.push(StepReport {
                    description,
                    elapsed_time,
                    output: res,
                });

                is_ok
            }
            Err(_) => false,
        }
    }

    pub async fn finish(mut self) -> ExecutorReport {
        let steps_ok = self.step_reports.iter().all(StepReport::success);
        match (self.ssh.as_mut(), steps_ok) {
            (Ok(ssh), true) => {
                let start = Instant::now();
                let res: Result<Result<(), io::Error>, Elapsed> =
                    time::timeout(self.config.poweroff_timeout, async {
                        ssh.exec(self.config.poweroff_command).await.ok();

                        while self.qemu.try_wait()?.is_none() {
                            time::sleep(Duration::from_millis(100)).await;
                        }

                        Ok(())
                    })
                    .await;
                let elapsed = start.elapsed();

                let output: crate::Result<Output> = match res {
                    Ok(Ok(_)) => Ok(Output::default()),
                    Ok(Err(error)) => {
                        self.qemu.kill().await.ok();
                        Err(error.into())
                    }
                    Err(error) => {
                        self.qemu.kill().await.ok();
                        Err(error.into())
                    }
                };

                self.step_reports.push(StepReport {
                    description: "shutdown QEMU".into(),
                    elapsed_time: elapsed,
                    output,
                })
            }
            _ => {
                self.qemu.kill().await.ok();
            }
        }

        let image = self.qemu.image_path().to_owned();
        let qemu = self.qemu.wait().await;

        ExecutorReport {
            image,
            qemu,
            connect: self.ssh.map(|_| ()),
            steps: self.step_reports,
        }
    }
}
