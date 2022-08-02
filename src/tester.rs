use crate::{
    executor::{stack::StackExecutor, ExecutorConfig, ExecutorReport},
    qemu::{Image, ImageBuilder, QemuSpawner},
    ssh::SshAction,
    Error,
};
use std::{
    collections::HashMap,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    fs,
    sync::mpsc::UnboundedSender,
    task::{self, JoinHandle},
};

#[derive(Debug)]
pub enum Step {
    Action {
        action: SshAction,
        timeout: Duration,
    },
    TransferPatch {
        to: PathBuf,
        mode: i32,
        timeout: Duration,
    },
}

impl Step {
    fn action(&self, patch: &Path) -> SshAction {
        match self {
            Self::Action { action, .. } => action.clone(),
            Self::TransferPatch { to, mode, .. } => SshAction::Send {
                from: patch.to_path_buf(),
                to: to.clone(),
                mode: *mode,
            },
        }
    }

    fn timeout(&self) -> Duration {
        match self {
            Self::Action { timeout, .. } => *timeout,
            Self::TransferPatch { timeout, .. } => *timeout,
        }
    }
}

#[derive(Default)]
pub struct Scenario {
    pub retries: usize,
    pub steps: Vec<Vec<Step>>,
}

pub struct RunConfig {
    pub execution: ExecutorConfig,
    pub build: Scenario,
    pub tests: HashMap<String, Scenario>,
}

#[derive(Default)]
pub struct ScenarioReport {
    images: Vec<PathBuf>,
    attempts: Vec<Vec<ExecutorReport>>,
}

impl ScenarioReport {
    fn push_attempt(&mut self, image: PathBuf, attempt: Vec<ExecutorReport>) {
        self.images.push(image);
        self.attempts.push(attempt);
    }

    fn last_image(&self) -> Option<&Path> {
        self.images.last().map(AsRef::as_ref)
    }

    fn err(&self) -> Option<&Error> {
        self.attempts.last()?.last()?.err()
    }
}

#[derive(Default)]
pub struct RunReport {
    build: ScenarioReport,
    tests: HashMap<String, ScenarioReport>,
}

async fn prepare_dir(path: &Path) -> io::Result<()> {
    if let Err(e) = fs::create_dir(path).await {
        if e.kind() != ErrorKind::AlreadyExists {
            return Err(e);
        }
    }

    Ok(())
}

type TestWorker = JoinHandle<Result<(String, ScenarioReport), Error>>;

pub struct PatchProcessor {
    pub spawner: QemuSpawner,
    pub builder: ImageBuilder,
    pub base_image: PathBuf,
    pub run_config: RunConfig,
}

impl PatchProcessor {
    async fn run_scenario(
        &self,
        patch: &Path,
        base_image: Image<'_>,
        artifacts: &Path,
        scenario: &Scenario,
    ) -> Result<ScenarioReport, Error> {
        let mut report = ScenarioReport::default();

        for i in 0..=scenario.retries {
            let dst = artifacts.join(format!("attempt_{}.img", i + 1));
            self.builder
                .create(base_image, Image::Qcow2(dst.as_ref()))
                .await?;

            let mut executor =
                StackExecutor::new(&self.run_config.execution, &self.spawner, dst.as_os_str());

            for phase in &scenario.steps {
                let iter = phase
                    .iter()
                    .map(|step| (step.action(patch), step.timeout()));

                let error = executor.open_stack().await?.consume(iter).await.is_err();
                if error {
                    break;
                }
            }

            let attempt = executor.finish();
            report.push_attempt(dst, attempt);

            if report.err().is_none() {
                break;
            }
        }

        Ok(report)
    }

    async fn build(&self, patch: &Path, artifacts_root: &Path) -> Result<ScenarioReport, Error> {
        let build_dir = artifacts_root.join("build");
        prepare_dir(&build_dir).await?;

        self.run_scenario(
            patch,
            Image::Raw(&self.base_image),
            &build_dir,
            &self.run_config.build,
        )
        .await
    }

    async fn spawn_test_workers(
        self: Arc<Self>,
        patch: &Path,
        artifacts_root: &Path,
        built_image: Option<&Path>,
    ) -> Result<Vec<TestWorker>, Error> {
        let tests_dir = artifacts_root.join("tests");
        prepare_dir(&tests_dir).await?;

        let handles = self
            .run_config
            .tests
            .keys()
            .cloned()
            .map(|test| {
                let tester = self.clone();
                let artifacts = tests_dir.join(&test);
                let image = built_image.map(Path::to_owned);
                let patch = patch.to_path_buf();

                task::spawn(async move {
                    prepare_dir(&artifacts).await?;

                    let base_image = image
                        .as_ref()
                        .map(AsRef::as_ref)
                        .map(Image::Qcow2)
                        .unwrap_or(Image::Raw(&tester.base_image));

                    let report = tester
                        .run_scenario(
                            &patch,
                            base_image,
                            &artifacts,
                            tester.run_config.tests.get(&test).unwrap(),
                        )
                        .await?;

                    Ok((test, report))
                })
            })
            .collect();

        Ok(handles)
    }

    pub async fn process(
        self: Arc<Self>,
        patch: &Path,
        artifacts_root: &Path,
    ) -> Result<RunReport, Error> {
        let artifacts_root = artifacts_root.canonicalize()?;

        let mut res = RunReport {
            build: self.build(patch, &artifacts_root).await?,
            tests: Default::default(),
        };
        if res.build.err().is_some() {
            return Ok(res);
        }

        let mut handles = self
            .spawn_test_workers(patch, &artifacts_root, res.build.last_image())
            .await?;

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

        Ok(res)
    }
}

#[derive(Clone)]
pub struct Tester {
    pub processor: Arc<PatchProcessor>,
    pub artifacts_root: PathBuf,
    pub reports_sink: UnboundedSender<(PathBuf, Result<RunReport, Error>)>,
}

impl Tester {
    pub async fn schedule(self, patch: PathBuf) -> io::Result<()> {
        let stem = patch
            .file_stem()
            .ok_or_else(|| io::Error::new(ErrorKind::Other, "path has no stem"))?;
        let artifacts = self.artifacts_root.join(stem);
        prepare_dir(&artifacts).await?;

        task::spawn(async move {
            let res = self.processor.process(&patch, &artifacts).await;
            self.reports_sink.send((patch, res))
        });

        Ok(())
    }
}
