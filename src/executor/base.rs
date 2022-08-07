use super::{ActionReport, ExecutorConfig, ExecutorReport};
use crate::{
    qemu::QemuInstance,
    ssh::{SshAction, SshHandle},
    Error,
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
                        SshHandle::new(
                            addr,
                            config.user.clone(),
                            config.password.clone(),
                            config.output_limit,
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
        if let Ok(ssh) = self.ssh.as_mut() {
            let action = SshAction::Exec {
                cmd: self.config.poweroff_command.clone(),
            };

            let res: Result<Result<(), io::Error>, Elapsed> =
                time::timeout(self.config.poweroff_timeout, async {
                    ssh.exec(action.clone()).await.ok();

                    while self.qemu.try_wait()?.is_none() {
                        time::sleep(Duration::from_millis(100)).await;
                    }

                    Ok(())
                })
                .await;

            match res {
                Ok(Err(_)) => {
                    self.qemu.kill().await.ok();
                }
                Err(_) => {
                    self.qemu.kill().await.ok();
                }
                _ => {}
            };
        } else {
            self.qemu.kill().await.ok();
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

#[cfg(test)]
mod test {
    use super::*;
    use crate::{qemu::Image, test_util::Env};

    async fn run_executor(
        config: &ExecutorConfig,
        actions: Vec<(SshAction, Duration)>,
    ) -> ExecutorReport {
        let env = Env::read();

        let image = env.base_path().join("image.qcow2");

        env.builder()
            .create(env.base_image(), Image::Qcow2(image.as_path()))
            .await
            .expect("failed to build the image");
        let qemu = env
            .spawner(1)
            .spawn(image.into())
            .await
            .expect("failed to spawn the QEMU process");

        let mut executor = BaseExecutor::new(qemu, config).await;

        for (action, timeout) in actions {
            executor.run(action, timeout).await.ok();
        }

        executor.finish().await
    }

    #[ignore]
    #[tokio::test]
    async fn ssh_timeout() {
        let config = ExecutorConfig {
            user: "root".into(),
            password: "root".into(),
            connection_timeout: Duration::from_secs(1),
            poweroff_timeout: Duration::from_secs(20),
            poweroff_command: "/sbin/poweroff".into(),
            output_limit: None,
        };
        let actions = vec![];

        let report = time::timeout(Duration::from_secs(10), run_executor(&config, actions))
            .await
            .expect("timeout");

        assert!(report.err().is_some());
        assert!(report.connect().is_some());
    }

    #[ignore]
    #[tokio::test]
    async fn faulty_command() {
        let config = ExecutorConfig {
            user: "root".into(),
            password: "root".into(),
            connection_timeout: Duration::from_secs(20),
            poweroff_timeout: Duration::from_secs(20),
            poweroff_command: "/sbin/poweroff".into(),
            output_limit: None,
        };
        let actions = vec![(
            SshAction::Exec {
                cmd: "idonotexist".into(),
            },
            Duration::from_secs(2),
        )];

        let report = time::timeout(Duration::from_secs(60), run_executor(&config, actions))
            .await
            .expect("timeout");

        assert!(report.err().is_some());
        assert!(report.connect().is_none());
        assert_eq!(report.action_reports().len(), 1);
        assert!(report.action_reports()[0].err().is_some());
        assert!(report.qemu().is_ok());
    }

    #[ignore]
    #[tokio::test]
    async fn invalid_poweroff() {
        let config = ExecutorConfig {
            user: "root".into(),
            password: "root".into(),
            connection_timeout: Duration::from_secs(20),
            poweroff_timeout: Duration::from_secs(20),
            poweroff_command: "/i/do/not/work".into(),
            output_limit: None,
        };
        let actions = vec![];

        let report = time::timeout(Duration::from_secs(60), run_executor(&config, actions))
            .await
            .expect("timeout");

        assert!(report.err().is_some());
        assert!(report.connect().is_none());
        assert!(report.qemu().is_err());
    }

    #[ignore]
    #[tokio::test]
    async fn all_good() {
        let config = ExecutorConfig {
            user: "root".into(),
            password: "root".into(),
            connection_timeout: Duration::from_secs(20),
            poweroff_timeout: Duration::from_secs(20),
            poweroff_command: "/sbin/poweroff".into(),
            output_limit: None,
        };
        let actions = vec![
            (
                SshAction::Exec { cmd: "pwd".into() },
                Duration::from_secs(1),
            ),
            (SshAction::Exec { cmd: "ls".into() }, Duration::from_secs(1)),
        ];

        let report = time::timeout(Duration::from_secs(60), run_executor(&config, actions))
            .await
            .expect("timeout");

        assert!(report.err().is_none());
        assert!(report.connect().is_none());
        assert_eq!(report.action_reports().len(), 2);
        assert!(report
            .action_reports()
            .iter()
            .all(|report| report.err().is_none()));
        assert!(report.qemu().is_ok());
    }
}
