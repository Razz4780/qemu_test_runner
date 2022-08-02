use super::{ActionReport, ExecutorConfig, ExecutorReport};
use crate::{
    qemu::QemuInstance,
    ssh::{SshAction, SshHandle},
    Error, Output,
};
use std::{
    io,
    time::{Duration, Instant},
};
use tokio::time::{self, error::Elapsed};

/// A wrapper over a [QemuInstance].
/// Used to interact with the instance over SSH.
pub struct BaseExecutor<'a> {
    qemu: QemuInstance,
    config: &'a ExecutorConfig,
    ssh: Result<SshHandle, Error>,
    reports: Vec<ActionReport>,
}

impl<'a> BaseExecutor<'a> {
    /// Creates a new instance of this struct.
    /// This instance will operate on the given [QemuInstance].
    pub async fn new(qemu: QemuInstance, config: &'a ExecutorConfig) -> BaseExecutor<'a> {
        let ssh = time::timeout(config.connection_timeout, async {
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
            reports: Default::default(),
        }
    }

    pub async fn run(&mut self, action: SshAction, timeout: Duration) -> Result<(), &Error> {
        let ssh = self.ssh.as_mut()?;

        let start = Instant::now();

        let res = time::timeout(timeout, ssh.exec(action.clone())).await;
        let elapsed_time = start.elapsed();
        let output = match res {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(error.into()),
        };

        self.reports.push(ActionReport {
            action,
            timeout,
            elapsed_time,
            output,
        });

        self.reports
            .last()
            .and_then(ActionReport::err)
            .map(Err)
            .unwrap_or(Ok(()))
    }

    pub async fn finish(mut self) -> ExecutorReport {
        let actions_ok = self
            .reports
            .last()
            .map(|report| report.output.is_ok())
            .unwrap_or(true);

        match (self.ssh.as_mut(), actions_ok) {
            (Ok(ssh), true) => {
                let action = SshAction::Exec {
                    cmd: self.config.poweroff_command.clone(),
                };

                let start = Instant::now();
                let res: Result<Result<(), io::Error>, Elapsed> =
                    time::timeout(self.config.poweroff_timeout, async {
                        ssh.exec(action.clone()).await.ok();

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

                self.reports.push(ActionReport {
                    action,
                    timeout: self.config.poweroff_timeout,
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
            action_reports: self.reports,
        }
    }
}
