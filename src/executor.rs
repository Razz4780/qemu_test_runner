use crate::{qemu::QemuInstance, ssh::SshHandle, CanFail, Result, Timeout};
use std::{
    cmp, io,
    os::unix::prelude::ExitStatusExt,
    path::PathBuf,
    process::{ExitStatus, Output},
    thread,
    time::Duration,
};

#[derive(Debug, Clone)]
pub enum Action {
    Send {
        local: PathBuf,
        remote: PathBuf,
        mode: i32,
        timeout: Duration,
    },
    Exec {
        cmd: String,
        timeout: Duration,
    },
}

pub struct ExecutionConfig {
    pub user: String,
    pub password: String,
    pub actions: Vec<Action>,
    pub startup_timeout: Duration,
    pub poweroff_timeout: Duration,
    pub poweroff_command: String,
}

#[derive(Debug)]
pub struct ExecutionReport {
    pub qemu: Result<Output>,
    pub connect: Result<()>,
    pub actions: Vec<Result<Output>>,
    pub poweroff: Option<Result<Output>>,
}

pub struct Executor {
    qemu: Option<QemuInstance>,
}

impl Executor {
    pub fn new(qemu: QemuInstance) -> Self {
        Self { qemu: Some(qemu) }
    }

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

    async fn kill_qemu(mut self) -> Result<Output> {
        let mut qemu = self.qemu.take().unwrap();
        qemu.kill().await.ok();
        qemu.wait().await?.result()
    }

    async fn wait_qemu(mut self, timeout: Duration) -> Result<Output> {
        let timeout = Timeout::new(timeout);

        loop {
            let exited = self.qemu.as_mut().unwrap().try_wait()?.is_some();
            if exited {
                break self.qemu.take().unwrap().wait().await?.result();
            }
            match timeout.remaining() {
                Ok(remaining) => thread::sleep(cmp::min(remaining, Duration::from_secs(1))),
                Err(_) => break self.kill_qemu().await,
            }
        }
    }

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
                        status: ExitStatus::from_raw(0),
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
