use super::{base::BaseExecutor, ExecutorConfig, ExecutorReport};
use crate::{qemu::QemuSpawner, ssh::SshAction, Error};
use std::{ffi::OsStr, time::Duration};

pub struct StackExecutor<'a> {
    config: &'a ExecutorConfig,
    reports: Vec<ExecutorReport>,
    spawner: &'a QemuSpawner,
    image: &'a OsStr,
}

impl<'a> StackExecutor<'a> {
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

    pub async fn open_stack(&mut self) -> Result<Stack<'_>, Error> {
        let qemu = self.spawner.spawn(self.image.to_owned()).await?;
        let inner = BaseExecutor::new(qemu, self.config).await;

        Ok(Stack {
            inner,
            reports: &mut self.reports,
        })
    }

    pub fn finish(self) -> Vec<ExecutorReport> {
        self.reports
    }
}

pub struct Stack<'a> {
    inner: BaseExecutor<'a>,
    reports: &'a mut Vec<ExecutorReport>,
}

impl<'a> Stack<'a> {
    pub async fn run(&mut self, action: SshAction, timeout: Duration) -> Result<(), &Error> {
        self.inner.run(action, timeout).await
    }

    pub async fn finish(self) -> Result<(), &'a Error> {
        let report = self.inner.finish().await;
        self.reports.push(report);

        self.reports
            .last()
            .and_then(ExecutorReport::err)
            .map(Err)
            .unwrap_or(Ok(()))
    }

    pub async fn consume<I>(mut self, iter: I) -> Result<(), &'a Error>
    where
        I: Iterator<Item = (SshAction, Duration)>,
    {
        for (action, timeout) in iter {
            if self.run(action, timeout).await.is_err() {
                break;
            }
        }

        self.finish().await
    }
}
