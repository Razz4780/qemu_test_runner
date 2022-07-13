use crate::{
    executor::{Action, Config, ExecutionReport, Executor},
    qemu::{Build, Forward, Instance, Run},
    CanFail, Timeout,
};
use std::{
    cmp,
    ffi::OsStr,
    fs,
    io::Result,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

pub struct QemuConfig {
    pub command: String,
    pub memory: u16,
    pub enable_kvm: bool,
    pub irqchip_off: bool,
    pub startup_timeout: Duration,
    pub poweroff_timeout: Duration,
    pub free_port_timeout: Duration,
    pub poweroff_command: String,
}

impl QemuConfig {
    fn spawn(&self, image: &OsStr) -> Result<(Instance, SocketAddr)> {
        let timeout = Timeout::new(self.free_port_timeout);
        let forward = loop {
            match Forward::new_ssh() {
                Some(forward) => break forward,
                None => {
                    let remaining = timeout.remaining()?;
                    thread::sleep(cmp::min(Duration::from_secs(1), remaining));
                }
            }
        };
        let addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), forward.from);

        let mut run = Run::default();
        run.irqchip(self.irqchip_off)
            .kvm(self.enable_kvm)
            .memory(self.memory)
            .forward(forward)
            .cmd(&self.command);
        let instance = run.spawn(image)?;

        Ok((instance, addr))
    }
}

pub struct ExecutionConfig {
    pub user: String,
    pub password: String,
    pub actions: Vec<Action>,
}

pub struct Builder {
    qemu_config: QemuConfig,
    base_image: PathBuf,
    build_steps: Vec<ExecutionConfig>,
    patch_dst: PathBuf,
}

impl Builder {
    fn try_build(&self, patch: &Path, image: &OsStr) -> Vec<Result<ExecutionReport>> {
        let mut reports = Vec::new();

        for (i, step) in self.build_steps.iter().enumerate() {
            let res = self.qemu_config.spawn(image).map(|(instance, ssh_addr)| {
                let executor = Executor::new(
                    instance,
                    Config {
                        ssh_addr,
                        ssh_username: step.user.clone(),
                        ssh_password: step.password.clone(),
                        startup_timeout: self.qemu_config.startup_timeout,
                        poweroff_timeout: self.qemu_config.poweroff_timeout,
                        poweroff_cmd: self.qemu_config.poweroff_command.clone(),
                    },
                );

                let mut actions = step.actions.clone();
                if i == 0 {
                    actions.insert(
                        0,
                        Action::Send {
                            local: patch.to_path_buf(),
                            remote: self.patch_dst.clone(),
                            mode: 0x777,
                            timeout: Duration::from_secs(2),
                        },
                    );
                }
                executor.run(actions)
            });

            reports.push(res);
            if reports.failed() {
                break;
            }
        }

        reports
    }

    pub fn run(&self, patch: &Path, artifacts: &Path) -> Result<Vec<Result<ExecutionReport>>> {
        let image = artifacts.join("minix.img");
        fs::copy(&self.base_image, &image)?;

        let reports = self.try_build(patch, image.as_os_str());
        if reports.failed() {
            fs::remove_file(&image)?;
        }

        Ok(reports)
    }
}

pub struct Runner {
    build_cmd: String,
    qemu_config: QemuConfig,
    base_image: PathBuf,
}

impl Runner {
    pub fn run(
        &self,
        artifacts: &Path,
        test_name: &str,
        test_config: &ExecutionConfig,
    ) -> Result<ExecutionReport> {
        let mut image = artifacts.join(test_name);
        image.set_extension("qcow2");
        let mut build = Build::default();
        let build_res = build
            .cmd(&self.build_cmd)
            .qcow2(self.base_image.as_os_str(), image.as_os_str())?;
        if build_res.failed() {
            todo!()
        }

        let (instance, ssh_addr) = self.qemu_config.spawn(image.as_os_str())?;

        let executor = Executor::new(
            instance,
            Config {
                ssh_addr,
                ssh_username: test_config.user.clone(),
                ssh_password: test_config.password.clone(),
                startup_timeout: self.qemu_config.startup_timeout,
                poweroff_timeout: self.qemu_config.poweroff_timeout,
                poweroff_cmd: self.qemu_config.poweroff_command.clone(),
            },
        );
        Ok(executor.run(test_config.actions.clone()))
    }
}
