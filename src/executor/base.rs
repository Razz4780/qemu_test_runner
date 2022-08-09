use super::{ActionReport, ExecutorConfig, ExecutorReport};
use crate::{
    qemu::QemuInstance,
    ssh::{SshAction, SshHandle},
    Output,
};
use std::{
    io,
    time::{Duration, Instant},
};
use tokio::time;

/// A wrapper over a [QemuInstance]. Used to run [SshAction]s and collect [ExecutorReport].
pub struct BaseExecutor<'a> {
    qemu: QemuInstance,
    config: &'a ExecutorConfig,
    ssh: Option<SshHandle>,
    reports: Vec<ActionReport>,
}

impl<'a> BaseExecutor<'a> {
    /// # Arguments
    /// * qemu - the QEMU process to wrap.
    /// * config - configuration for SSH and timeouts.
    /// # Returns
    /// A new instance of this struct.
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
                    Err(e) => Err(e),
                };

                if let Ok(handle) = handle {
                    break handle;
                }

                time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .ok();

        if ssh.is_some() {
            log::debug!(
                "Established an SSH connection to the QEMU instance [{}].",
                qemu.image_path().to_string_lossy()
            );
        }

        Self {
            qemu,
            config,
            ssh,
            reports: Default::default(),
        }
    }

    /// # Arguments
    /// * action - an [SshAction] to run on the wrapped QEMU process.
    /// * timeout - a timeout for this action.
    /// # Returns
    /// Whether the execution was successful.
    pub async fn run(&mut self, action: SshAction, timeout: Duration) -> io::Result<bool> {
        let ssh = match self.ssh.as_mut() {
            Some(ssh) => ssh,
            None => return Ok(false),
        };

        let start = Instant::now();
        let res = time::timeout(timeout, ssh.exec(action.clone())).await;
        let elapsed_time = start.elapsed();

        let output = match res {
            Ok(res) => res?,
            Err(_) => Output::Timeout,
        };
        let success = output.success();

        let report = ActionReport {
            action,
            timeout_ms: timeout.as_millis(),
            elapsed_time_ms: elapsed_time.as_millis(),
            output,
        };
        log::debug!(
            "Executed an action {:?} on the QEMU instance [{}].",
            report,
            self.qemu.image_path().to_string_lossy()
        );
        self.reports.push(report);

        Ok(success)
    }

    /// Executes a poweroff command (configured with the [ExecutorConfig]) on the wrapped QEMU process
    /// and waits for the process to exit.
    /// # Returns
    /// A report from all [SshAction]s performed through this struct.
    pub async fn finish(mut self) -> io::Result<ExecutorReport> {
        let image = self.qemu.image_path().to_os_string();

        let (ssh_ok, exit_ok) = match self.ssh.as_mut() {
            Some(ssh) => {
                log::debug!(
                    "Executing a poweroff command '{}' on the QEMU instance [{}].",
                    self.config.poweroff_command,
                    image.to_string_lossy()
                );
                let action = SshAction::Exec {
                    cmd: self.config.poweroff_command.clone(),
                };

                let res: Result<Result<_, io::Error>, _> =
                    time::timeout(self.config.poweroff_timeout, async {
                        ssh.exec(action.clone()).await?;

                        while self.qemu.try_wait()?.is_none() {
                            time::sleep(Duration::from_millis(100)).await;
                        }

                        Ok(())
                    })
                    .await;

                match res {
                    Ok(Ok(_)) => {
                        log::debug!("QEMU process [{}] exited on time.", image.to_string_lossy());
                        self.qemu.wait().await?;
                        (true, true)
                    }
                    Ok(Err(error)) => return Err(error),
                    Err(_) => {
                        log::debug!(
                            "QEMU process [{}] did not exit on time, killing the process.",
                            image.to_string_lossy()
                        );
                        self.qemu.kill().await.ok();
                        self.qemu.wait().await.ok();
                        (true, false)
                    }
                }
            }
            None => {
                self.qemu.kill().await.ok();
                self.qemu.wait().await.ok();
                (false, false)
            }
        };

        Ok(ExecutorReport {
            image: image.into(),
            ssh_ok,
            action_reports: self.reports,
            exit_ok,
        })
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
            executor.run(action, timeout).await.unwrap();
        }

        executor.finish().await.unwrap()
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

        assert!(!report.success());
        assert!(!report.ssh_ok);
        assert!(report.action_reports.is_empty());
        assert!(!report.exit_ok);
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

        assert!(!report.success());
        assert!(report.ssh_ok);
        assert_eq!(report.action_reports().len(), 1);
        assert!(!report.action_reports()[0].success());
        assert!(report.exit_ok);
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

        assert!(!report.success());
        assert!(report.ssh_ok);
        assert!(report.action_reports.is_empty());
        assert!(!report.exit_ok);
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

        assert!(report.success());
        assert!(report.ssh_ok);
        assert_eq!(report.action_reports.len(), 2);
        assert!(report.action_reports.iter().all(|report| report.success()));
        assert!(report.exit_ok);
    }
}
