use super::{base::BaseExecutor, Config, ExecutorReport};
use crate::{qemu::QemuSpawner, ssh::SshCommand, Error};
use std::{ffi::OsStr, sync::Arc, time::Duration};

pub struct StackExecutor<'a> {
    config: &'a Config,
    reports: Vec<ExecutorReport>,
    spawner: &'a QemuSpawner,
    image: &'a OsStr,
}

impl<'a> StackExecutor<'a> {
    pub async fn new(
        config: &'a Config,
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

    pub async fn open_stack(&'a mut self) -> Result<Stack<'a>, Error> {
        let qemu = self.spawner.spawn(self.image.to_owned()).await?;
        let inner = BaseExecutor::new(qemu, self.config).await;

        Ok(Stack {
            inner,
            reports: &mut self.reports,
        })
    }

    pub async fn finish(self) -> Vec<ExecutorReport> {
        self.reports
    }
}

pub struct Stack<'a> {
    inner: BaseExecutor<'a>,
    reports: &'a mut Vec<ExecutorReport>,
}

impl<'a> Stack<'a> {
    pub async fn run(&mut self, step: Arc<SshCommand>, timeout: Duration) -> Result<(), &Error> {
        self.inner.run(step, timeout).await
    }

    pub async fn finish(self) {
        let report = self.inner.finish().await;
        self.reports.push(report);
    }
}
