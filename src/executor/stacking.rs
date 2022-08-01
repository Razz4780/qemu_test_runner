use super::{base::Executor as BaseExecutor, Config, ExecutorReport};
use crate::{qemu::QemuInstance, ssh::SshCommand, Error};
use std::{mem, sync::Arc, time::Duration};

pub struct Executor<'a> {
    config: &'a Config,
    inner: BaseExecutor<'a>,
    reports: Vec<ExecutorReport>,
}

impl<'a> Executor<'a> {
    pub async fn new(qemu: QemuInstance, config: &'a Config) -> Executor<'a> {
        Self {
            config,
            inner: BaseExecutor::new(qemu, config).await,
            reports: Default::default(),
        }
    }

    pub async fn run(&mut self, cmd: Arc<SshCommand>, timeout: Duration) -> Result<(), &Error> {
        self.inner.run(cmd, timeout).await
    }

    pub async fn finish(mut self) -> Vec<ExecutorReport> {
        self.reports.push(self.inner.finish().await);
        self.reports
    }

    pub async fn next_stack(&mut self, qemu: QemuInstance) -> Result<(), &Error> {
        let mut tmp = BaseExecutor::new(qemu, self.config).await;
        mem::swap(&mut tmp, &mut self.inner);
        self.reports.push(tmp.finish().await);

        self.reports
            .last()
            .map(ExecutorReport::ok)
            .transpose()
            .map(Option::unwrap_or_default)
    }
}
