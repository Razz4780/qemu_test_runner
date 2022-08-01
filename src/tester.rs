use crate::{
    executor::{
        base::BaseExecutor, stack::StackExecutor, Config as ExecutorConfig, ExecutorReport,
    },
    qemu::{Image, ImageBuilder, QemuSpawner},
    ssh::SshCommand,
    DurationMs, Error,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::task::{self, JoinHandle};

#[derive(Deserialize)]
pub struct RunConfig {
    pub execution: ExecutorConfig,
    pub patch_dst: PathBuf,
    pub build: Config,
    pub tests: HashMap<String, Config>,
}

#[derive(Default)]
pub struct PartialReport {
    inner: Vec<ExecutorReport>,
}

impl PartialReport {
    fn push(&mut self, report: ExecutorReport) {
        self.inner.push(report)
    }

    fn ok(&self) -> Result<(), &Error> {
        self.inner
            .last()
            .map(ExecutorReport::ok)
            .transpose()
            .map(Option::unwrap_or_default)
    }

    fn join(&mut self, other: PartialReport) {
        self.inner.extend(other.inner)
    }
}

pub struct TestReport {
    pub solution: PathBuf,
    pub build: Vec<PartialReport>,
    pub tests: HashMap<String, Vec<PartialReport>>,
}

impl TestReport {
    fn new(solution: PathBuf) -> Self {
        Self {
            solution,
            build: Default::default(),
            tests: Default::default(),
        }
    }
}

#[derive(Deserialize, Clone)]
pub struct Config {
    retries: usize,
    phases: Vec<Vec<(SshCommand, DurationMs)>>,
}

pub struct Tester {
    pub spawner: QemuSpawner,
    pub builder: ImageBuilder,
    pub base_image: PathBuf,
    pub run_config: RunConfig,
}

impl Tester {
    async fn try_run(&self, image: &OsStr, config: &Config) -> crate::Result<PartialReport> {
        let mut executor = StackExecutor::new(&self.run_config.execution, &self.spawner, image);

        for phase in config.phases.iter().cloned() {
            let mut stack = executor.open_stack().await?;

            for (step, timeout) in phase {
                let stop = stack.run(step.into(), timeout.into()).await.is_err();
                if stop {
                    break;
                }
            }

            if stack.finish().await.is_err() {
                break;
            }
        }

        Ok(PartialReport {
            inner: executor.finish(),
        })
    }

    async fn try_build(&self, solution: PathBuf, image: &OsStr) -> crate::Result<PartialReport> {
        let mut res = PartialReport::default();

        let instance = self.spawner.spawn(image.to_owned()).await?;
        let mut executor = BaseExecutor::new(instance, &self.run_config.execution).await;
        executor
            .run(
                Arc::new(SshCommand::Send {
                    from: solution,
                    to: self.run_config.patch_dst.clone(),
                    mode: 0o777,
                }),
                Duration::from_secs(2),
            )
            .await
            .ok();
        res.push(executor.finish().await);

        if res.ok().is_err() {
            return Ok(res);
        }

        res.join(self.try_run(image, &self.run_config.build).await?);

        Ok(res)
    }

    async fn try_test(&self, test: &str, image: &OsStr) -> crate::Result<PartialReport> {
        let config = self.run_config.tests.get(test).unwrap();
        self.try_run(image, config).await
    }

    pub async fn process(
        self: Arc<Self>,
        solution: &Path,
        artifacts: Arc<PathBuf>,
    ) -> crate::Result<TestReport> {
        let mut res = TestReport::new(solution.to_path_buf());

        let patched_img = Arc::new(artifacts.join("patched.img"));
        let mut success = false;
        for _ in 0..=self.run_config.build.retries {
            self.builder
                .create(Image::Raw(&self.base_image), Image::Qcow2(&patched_img))
                .await?;
            let report = self
                .try_build(solution.to_path_buf(), patched_img.as_os_str())
                .await?;

            success = report.ok().is_ok();
            res.build.push(report);

            if success {
                break;
            }
        }

        if success {
            let mut handles = Vec::with_capacity(self.run_config.tests.len());

            for test in self.run_config.tests.keys().cloned() {
                let tester = self.clone();
                let artifacts = artifacts.clone();
                let patched_img = patched_img.clone();

                let handle = task::spawn(async move {
                    let mut reports = Vec::new();

                    let config = tester.run_config.tests.get(&test).unwrap();

                    let mut success;
                    for i in 0..=config.retries {
                        let img = artifacts.join(format!("{}_attempt_{}.img", test, i + 1));
                        let build_res = tester
                            .builder
                            .create(Image::Qcow2(&patched_img), Image::Qcow2(&img))
                            .await;
                        if let Err(e) = build_res {
                            return Err(e);
                        }

                        let res = tester.try_test(&test, img.as_os_str()).await;
                        match res {
                            Ok(report) => {
                                success = report.ok().is_ok();
                                reports.push(report);

                                if success {
                                    break;
                                }
                            }
                            Err(e) => return Err(e),
                        }
                    }

                    Ok((test, reports))
                });

                handles.push(handle);
            }

            while let Some(handle) = handles.pop() {
                match handle.await {
                    Ok(Ok((test, reports))) => {
                        res.tests.insert(test, reports);
                    }
                    Ok(Err(error)) => {
                        handles.iter().for_each(JoinHandle::abort);
                        return Err(error);
                    }
                    Err(error) => {
                        handles.iter().for_each(JoinHandle::abort);
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            format!("failed to run test: {}", error),
                        )
                        .into());
                    }
                }
            }
        }

        Ok(res)
    }
}
