use crate::{
    executor::{ExecutionConfig, Executor, ExecutorReport},
    qemu::{Image, ImageBuilder, QemuSpawner},
};
use std::{
    collections::HashMap,
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::task::{self, JoinHandle};

#[derive(Default)]
pub struct PartialReport {
    inner: Vec<ExecutorReport>,
}

impl PartialReport {
    fn push(&mut self, report: ExecutorReport) {
        self.inner.push(report)
    }

    fn success(&self) -> bool {
        self.inner.iter().all(ExecutorReport::success)
    }

    fn join(&mut self, other: PartialReport) {
        self.inner.extend(other.inner)
    }
}

#[derive(Default)]
pub struct TestReport {
    build: Vec<PartialReport>,
    tests: HashMap<String, Vec<PartialReport>>,
}

#[derive(Clone)]
pub enum Step {
    Cmd {
        command: String,
        timeout: Duration,
    },
    Send {
        local: PathBuf,
        remote: PathBuf,
        mode: i32,
        timeout: Duration,
    },
}

pub struct Config {
    retries: usize,
    config: ExecutionConfig,
    phases: Vec<Vec<Step>>,
}

pub struct Tester {
    pub spawner: QemuSpawner,
    pub builder: ImageBuilder,
    pub base_image: PathBuf,
    pub build_config: Config,
    pub tests: HashMap<String, Config>,
    pub patch_dst: PathBuf,
}

impl Tester {
    async fn try_run(&self, image: &OsStr, config: &Config) -> crate::Result<PartialReport> {
        let mut res = PartialReport::default();

        for phase in config.phases.iter().cloned() {
            let instance = self.spawner.spawn(image.to_owned()).await?;
            let mut executor = Executor::new(instance, &self.build_config.config).await;

            for step in phase {
                let cont = match step {
                    Step::Cmd { command, timeout } => executor.execute(command, timeout).await,
                    Step::Send {
                        local,
                        remote,
                        mode,
                        timeout,
                    } => executor.send(local, remote, mode, timeout).await,
                };

                if !cont {
                    break;
                }
            }

            res.push(executor.finish().await);
            if !res.success() {
                break;
            }
        }

        Ok(res)
    }

    async fn try_build(&self, solution: PathBuf, image: &OsStr) -> crate::Result<PartialReport> {
        let mut res = PartialReport::default();

        let instance = self.spawner.spawn(image.to_owned()).await?;
        let mut executor = Executor::new(instance, &self.build_config.config).await;
        executor
            .send(
                solution,
                self.patch_dst.clone(),
                0o777,
                Duration::from_secs(2),
            )
            .await;
        res.push(executor.finish().await);

        if !res.success() {
            return Ok(res);
        }

        res.join(self.try_run(image, &self.build_config).await?);

        Ok(res)
    }

    async fn try_test(&self, test: &str, image: &OsStr) -> crate::Result<PartialReport> {
        let config = self.tests.get(test).unwrap();
        self.try_run(image, config).await
    }

    pub async fn process(
        self: Arc<Self>,
        solution: &Path,
        artifacts: Arc<PathBuf>,
    ) -> crate::Result<TestReport> {
        let mut res = TestReport::default();

        let patched_img = Arc::new(artifacts.join("patched.img"));
        let mut success = false;
        for _ in 0..=self.build_config.retries {
            self.builder
                .create(Image::Raw(&self.base_image), Image::Qcow2(&patched_img))
                .await?;
            let report = self
                .try_build(solution.to_path_buf(), patched_img.as_os_str())
                .await?;

            success = report.success();
            res.build.push(report);

            if success {
                break;
            }
        }

        if success && !self.tests.is_empty() {
            let mut handles = Vec::with_capacity(self.tests.len());

            for test in self.tests.keys().cloned() {
                let tester = self.clone();
                let artifacts = artifacts.clone();
                let patched_img = patched_img.clone();

                let handle = task::spawn(async move {
                    let mut reports = Vec::new();

                    let config = tester.tests.get(&test).unwrap();

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
                                success = report.success();
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
