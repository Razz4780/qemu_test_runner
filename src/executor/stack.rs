use super::{base::BaseExecutor, ExecutorConfig, ExecutorReport};
use crate::{qemu::QemuSpawner, ssh::SshAction};
use std::{ffi::OsStr, io, time::Duration};

/// A struct used to execute multiple stacks of [SshAction]s on a QEMU image
/// with reboots in-between stacks.
pub struct StackExecutor<'a> {
    config: &'a ExecutorConfig,
    reports: Vec<ExecutorReport>,
    spawner: &'a QemuSpawner,
    image: &'a OsStr,
}

impl<'a> StackExecutor<'a> {
    /// # Arguments
    /// * config - configuration for SSH and timeouts.
    /// * spawner - used to spawn new [crate::qemu::QemuInstance]s.
    /// * image - path to the image to operate on.
    /// # Returns
    /// A new instance of this struct.
    pub fn new(
        config: &'a ExecutorConfig,
        spawner: &'a QemuSpawner,
        image: &'a OsStr,
    ) -> StackExecutor<'a> {
        Self {
            config,
            reports: Default::default(),
            spawner,
            image,
        }
    }

    /// Opens a new stack. This includes spawning a new QEMU process.
    /// # Returns
    /// The newly opened stack.
    pub async fn open_stack(&mut self) -> io::Result<Stack<'_>> {
        let qemu = self.spawner.spawn(self.image.to_owned()).await?;
        let inner = BaseExecutor::new(qemu, self.config).await;

        Ok(Stack {
            inner,
            reports: &mut self.reports,
        })
    }

    /// # Returns
    /// Reports from all [Stack]s opened through this struct.
    pub fn finish(self) -> Vec<ExecutorReport> {
        self.reports
    }
}

/// A struct used to execute [SshAction]s on a QEMU image.
/// Basically a wrapper over a [BaseExecutor].
pub struct Stack<'a> {
    inner: BaseExecutor<'a>,
    reports: &'a mut Vec<ExecutorReport>,
}

impl<'a> Stack<'a> {
    /// # Arguments
    /// * action - an [SshAction] to run through the wrapped [BaseExecutor].
    /// * timeout - a timeout for this action.
    /// # Returns
    /// Whether the execution was successful.
    pub async fn run(&mut self, action: SshAction, timeout: Duration) -> io::Result<bool> {
        self.inner.run(action, timeout).await
    }

    /// Finishes the wrapped [BaseExecutor].
    /// # Returns
    /// Whether all [SshAction]s performed with the wrapped [BaseExecutor] were successful.
    pub async fn finish(self) -> io::Result<bool> {
        let report = self.inner.finish().await?;
        let success = report.success();
        self.reports.push(report);

        Ok(success)
    }

    /// Runs the given [SshAction]s until one of them is not successful
    /// and finishes the wrapped [BaseExecutor].
    /// # Arguments
    /// iter - an iterator of [SshAction]s to run and their timeouts.
    /// # Returns
    /// Whether all [SshAction]s performed with the wrapped [BaseExecutor] were successful.
    pub async fn run_until_failure<I>(mut self, iter: I) -> io::Result<bool>
    where
        I: Iterator<Item = (SshAction, Duration)>,
    {
        for (action, timeout) in iter {
            if !self.run(action, timeout).await? {
                break;
            }
        }

        self.finish().await
    }
}

#[cfg(test)]
mod test {
    use tokio::time;

    use super::*;
    use crate::{qemu::Image, test_util::Env};

    #[ignore]
    #[tokio::test]
    async fn persistent_changes() {
        let env = Env::read();

        let image = env.base_path().join("image.qcow2");

        env.builder()
            .create(env.base_image(), Image::Qcow2(image.as_path()))
            .await
            .expect("failed to build the image");
        let spawner = env.spawner(1);

        let config = ExecutorConfig {
            user: "root".into(),
            password: "root".into(),
            connection_timeout: Duration::from_secs(20),
            poweroff_timeout: Duration::from_secs(20),
            poweroff_command: "/sbin/poweroff".into(),
            output_limit: None,
        };

        let reports = time::timeout(Duration::from_secs(180), async {
            let mut executor = StackExecutor::new(&config, &spawner, image.as_os_str());

            let mut stack = executor.open_stack().await.expect("failed to open_stack");
            let success = stack
                .run(
                    SshAction::Exec {
                        cmd: "touch file1".into(),
                    },
                    Duration::from_secs(1),
                )
                .await
                .unwrap();
            assert!(success);
            let success = stack.finish().await.unwrap();
            assert!(success);

            let mut stack = executor.open_stack().await.expect("failed to open_stack");
            let success = stack
                .run(
                    SshAction::Exec {
                        cmd: "cat file1".into(),
                    },
                    Duration::from_secs(1),
                )
                .await
                .unwrap();
            assert!(success);
            let success = stack
                .run(
                    SshAction::Exec {
                        cmd: "rm file1".into(),
                    },
                    Duration::from_secs(1),
                )
                .await
                .unwrap();
            assert!(success);
            let success = stack
                .run(
                    SshAction::Exec {
                        cmd: "touch file2".into(),
                    },
                    Duration::from_secs(1),
                )
                .await
                .unwrap();
            assert!(success);
            let success = stack.finish().await.unwrap();
            assert!(success);

            let mut stack = executor.open_stack().await.expect("failed to open_stack");
            let success = stack
                .run(
                    SshAction::Exec {
                        cmd: "cat file2".into(),
                    },
                    Duration::from_secs(1),
                )
                .await
                .unwrap();
            assert!(success);
            let success = stack.finish().await.unwrap();
            assert!(success);

            let mut stack = executor.open_stack().await.expect("failed to open_stack");
            let success = stack
                .run(
                    SshAction::Exec {
                        cmd: "cat file3".into(),
                    },
                    Duration::from_secs(1),
                )
                .await
                .unwrap();
            assert!(!success);
            let success = stack.finish().await.unwrap();
            assert!(!success);

            executor.finish()
        })
        .await
        .expect("timeout");

        assert_eq!(reports.len(), 4);
        assert!(reports[0].success());
        assert!(reports[1].success());
        assert!(reports[2].success());
        assert!(!reports[3].success());
    }
}
