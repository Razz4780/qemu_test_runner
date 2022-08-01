use super::{Config, StepReport};
use crate::{
    qemu::QemuInstance,
    ssh::{SshCommand, SshHandle},
    Error, Output,
};
use std::{
    ffi::{OsStr, OsString},
    io,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::time::{self, error::Elapsed};

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

    pub fn success(&self) -> bool {
        self.qemu.is_ok() && self.connect.is_ok() && self.steps.iter().all(StepReport::success)
    }
}

/// A wrapper over a [QemuInstance].
/// Used to interact with the instance over SSH.
pub struct Executor<'a> {
    qemu: QemuInstance,
    config: &'a Config,
    ssh: Result<SshHandle, Error>,
    step_reports: Vec<StepReport>,
}

impl<'a> Executor<'a> {
    /// Creates a new instance of this struct.
    /// This instance will operate on the given [QemuInstance].
    pub async fn new(qemu: QemuInstance, config: &'a Config) -> Executor<'a> {
        let ssh = time::timeout(config.connection_timeout.into(), async {
            loop {
                let handle = match qemu.ssh().await {
                    Ok(addr) => {
                        SshHandle::new(addr, config.user.clone(), config.password.clone()).await
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
        .map_err(io::Error::from)
        .map_err(Into::into);

        Self {
            qemu,
            config,
            ssh,
            step_reports: Default::default(),
        }
    }

    pub async fn run(&mut self, step: Arc<SshCommand>, timeout: Duration) -> Result<(), &Error> {
        match self.ssh.as_mut() {
            Ok(ssh) => {
                let start = Instant::now();

                let res = time::timeout(timeout, ssh.exec(step.clone())).await;
                let elapsed_time = start.elapsed();
                let output = match res {
                    Ok(Ok(output)) => Ok(output),
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(error.into()),
                };

                self.step_reports.push(StepReport {
                    cmd: step,
                    timeout,
                    elapsed_time,
                    output,
                });

                self.step_reports
                    .last()
                    .map(|report| report.output.as_ref())
                    .transpose()
                    .map(|_| ())
            }
            Err(e) => Err(e),
        }
    }

    pub async fn finish(mut self) -> ExecutorReport {
        let steps_ok = self
            .step_reports
            .last()
            .map(|report| report.output.is_ok())
            .unwrap_or(true);

        match (self.ssh.as_mut(), steps_ok) {
            (Ok(ssh), true) => {
                let cmd = Arc::new(SshCommand::Exec {
                    cmd: self.config.poweroff_command.clone(),
                });

                let start = Instant::now();
                let res: Result<Result<(), io::Error>, Elapsed> =
                    time::timeout(self.config.poweroff_timeout.into(), async {
                        ssh.exec(cmd.clone()).await.ok();

                        while self.qemu.try_wait()?.is_none() {
                            time::sleep(Duration::from_millis(100)).await;
                        }

                        Ok(())
                    })
                    .await;
                let elapsed = start.elapsed();

                let output: Result<Output, Error> = match res {
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
                    cmd,
                    timeout: self.config.poweroff_timeout.into(),
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
