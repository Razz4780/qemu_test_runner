use crate::{qemu::Instance, ssh::Controller, CanFail, Timeout};
use std::{
    cmp,
    ffi::OsString,
    io::Result,
    net::SocketAddr,
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

pub struct Config {
    pub ssh_addr: SocketAddr,
    pub ssh_username: String,
    pub ssh_password: String,
    pub startup_timeout: Duration,
    pub poweroff_timeout: Duration,
    pub poweroff_cmd: String,
}

#[derive(Debug)]
pub struct ExecutionReport {
    pub image_path: OsString,
    pub qemu: Result<Output>,
    pub connect: Result<()>,
    pub actions: Vec<(Action, Result<Output>)>,
    pub poweroff: Option<Result<Output>>,
}

impl CanFail for ExecutionReport {
    fn failed(&self) -> bool {
        self.qemu.failed()
            || self.connect.failed()
            || self.actions.iter().any(|(_, res)| res.failed())
    }
}

pub struct Executor {
    qemu: Option<Instance>,
    config: Config,
}

impl Executor {
    pub fn new(qemu: Instance, config: Config) -> Self {
        Self {
            qemu: Some(qemu),
            config,
        }
    }

    fn controller(&self) -> Result<Controller> {
        Controller::new(
            self.config.ssh_addr,
            &self.config.ssh_username,
            &self.config.ssh_password,
            self.config.startup_timeout,
        )
    }

    fn kill_qemu(mut self) -> Result<Output> {
        let mut qemu = self.qemu.take().unwrap();
        qemu.kill().ok();
        qemu.wait()
    }

    fn wait_qemu(mut self) -> Result<Output> {
        let timeout = Timeout::new(self.config.poweroff_timeout);

        loop {
            let exited = self.qemu.as_mut().unwrap().try_wait()?.is_some();
            if exited {
                break self.qemu.take().unwrap().wait();
            }
            match timeout.remaining() {
                Ok(remaining) => thread::sleep(cmp::min(remaining, Duration::from_secs(1))),
                Err(_) => break self.kill_qemu(),
            }
        }
    }

    pub fn run(self, actions: Vec<Action>) -> ExecutionReport {
        let image_path = self.qemu.as_ref().unwrap().image().into();

        let mut controller = match self.controller() {
            Ok(controller) => controller,
            Err(e) => {
                return ExecutionReport {
                    image_path,
                    qemu: self.kill_qemu(),
                    connect: Err(e),
                    actions: Default::default(),
                    poweroff: None,
                }
            }
        };

        let mut results = Vec::with_capacity(actions.len());
        let mut err = false;
        for action in actions {
            let res = match &action {
                Action::Exec { cmd, timeout } => controller.exec(cmd, *timeout),
                Action::Send {
                    local,
                    remote,
                    mode,
                    timeout,
                } => controller
                    .send(local, remote, *mode, *timeout)
                    .map(|_| Output {
                        status: ExitStatus::from_raw(0),
                        stdout: Default::default(),
                        stderr: Default::default(),
                    }),
            };
            err = res.failed();

            results.push((action, res));

            if err {
                break;
            }
        }

        let (qemu, poweroff) = if err {
            (self.kill_qemu(), None)
        } else {
            let poweroff = controller.exec(&self.config.poweroff_cmd, self.config.poweroff_timeout);
            (self.wait_qemu(), Some(poweroff))
        };

        ExecutionReport {
            image_path,
            qemu,
            connect: Ok(()),
            actions: results,
            poweroff,
        }
    }
}
