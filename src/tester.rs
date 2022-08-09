use crate::{
    executor::{stack::StackExecutor, ExecutorConfig, ExecutorReport},
    patch_validator::Patch,
    qemu::{Image, ImageBuilder, QemuSpawner},
    ssh::SshAction,
};
use serde::Serialize;
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

/// A single step during building or testing.
#[derive(Debug)]
pub enum Step {
    /// Executing an [SshAction].
    Action {
        /// Action to execute.
        action: SshAction,
        /// Timeout for this action.
        timeout: Duration,
    },
    /// Transfering the solution to the guest machine.
    TransferPatch {
        /// Path to the destination file on the guest machine.
        to: PathBuf,
        /// Permissions of the destination file one the guest machine.
        mode: i32,
        /// Timeout for this transfer.
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

/// A scenario for the build process or a single test.
#[derive(Debug, Default)]
pub struct Scenario {
    /// Number of allowed retries.
    pub retries: usize,
    /// Stacks of [Step]s to execute with reboots in-between.
    pub steps: Vec<Vec<Step>>,
}

/// A config for the whole build-and-test process.
#[derive(Debug)]
pub struct RunConfig {
    /// Common configuration for the whole process.
    pub execution: ExecutorConfig,
    /// Build process configuration.
    pub build: Scenario,
    /// Test configurations.
    pub tests: HashMap<String, Scenario>,
}

/// A report from a single [Scenario].
#[derive(Default, Serialize)]
pub struct ScenarioReport {
    /// Images used for all attempts.
    images: Vec<PathBuf>,
    /// Reports from all attempts.
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

    /// # Returns
    /// Whether the scenario was successful.
    pub fn success(&self) -> bool {
        self.attempts
            .last()
            .map(|reports| reports.iter().all(ExecutorReport::success))
            .unwrap_or(true)
    }
}

/// A report from the whole build-and-test process.
#[derive(Serialize)]
pub struct RunReport {
    build: ScenarioReport,
    tests: HashMap<String, ScenarioReport>,
}

impl RunReport {
    /// # Returns
    /// Report from the build scenario.
    pub fn build(&self) -> &ScenarioReport {
        &self.build
    }

    /// # Returns
    /// Reports from the test scenarios.
    pub fn tests(&self) -> &HashMap<String, ScenarioReport> {
        &self.tests
    }
}

async fn prepare_dir(path: &Path) -> io::Result<()> {
    if let Err(e) = fs::create_dir(path).await {
        if e.kind() != ErrorKind::AlreadyExists {
            return Err(e);
        }
    }

    Ok(())
}

type TestWorker = JoinHandle<io::Result<(String, ScenarioReport)>>;

/// A struct for executing build-and-test processes on [Patch]es.
pub struct PatchProcessor {
    /// The spawner which will be used to create new QEMU processes.
    pub spawner: QemuSpawner,
    /// The builder which will be user to create new QEMU images.
    pub builder: ImageBuilder,
    /// Path to the base QEMU image.
    pub base_image: PathBuf,
    /// Configuration for the process.
    pub run_config: RunConfig,
}

impl PatchProcessor {
    async fn run_scenario(
        &self,
        patch: &Path,
        base_image: Image<'_>,
        artifacts: &Path,
        scenario: &Scenario,
    ) -> io::Result<ScenarioReport> {
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

                let success = executor.open_stack().await?.run_until_failure(iter).await?;
                if !success {
                    break;
                }
            }

            let attempt = executor.finish();
            report.push_attempt(dst, attempt);

            if report.success() {
                break;
            }
        }

        Ok(report)
    }

    async fn build(&self, patch: &Path, artifacts_root: &Path) -> io::Result<ScenarioReport> {
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
    ) -> io::Result<Vec<TestWorker>> {
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

    /// Executes the build-and-test process for a single patch.
    /// # Arguments
    /// patch - path to the processed solution.
    /// artifacts_root - root directory for artifacts from the process.
    /// # Returns
    /// A [RunReport] from the process.
    pub async fn process(
        self: Arc<Self>,
        patch: &Path,
        artifacts_root: &Path,
    ) -> io::Result<RunReport> {
        log::info!("Building a test image for patch at {}.", patch.display());
        let mut res = RunReport {
            build: self.build(patch, artifacts_root).await?,
            tests: Default::default(),
        };
        if !res.build.success() {
            log::info!("Build process failed for patch {}.", patch.display());
            return Ok(res);
        }

        log::info!("Spawning test workers for patch {}.", patch.display());
        let mut handles = self
            .spawn_test_workers(patch, artifacts_root, res.build.last_image())
            .await?;

        while let Some(handle) = handles.pop() {
            match handle.await {
                Ok(Ok((test, reports))) => {
                    log::info!(
                        "Received report from test {} for patch {}.",
                        test,
                        patch.display()
                    );
                    res.tests.insert(test, reports);
                }
                Ok(Err(error)) => {
                    log::error!(
                        "An error occurred when running tests for patch {}. Error: {}.",
                        patch.display(),
                        error
                    );
                    handles.iter().for_each(JoinHandle::abort);

                    return Err(error);
                }
                Err(error) => {
                    log::error!(
                        "Internal task panicked when running tests for patch {}. Error: {}.",
                        patch.display(),
                        error
                    );
                    handles.iter().for_each(JoinHandle::abort);

                    return Err(io::Error::new(io::ErrorKind::Other, error));
                }
            }
        }

        Ok(res)
    }
}

/// A struct for scheduling build-and-test processes.
#[derive(Clone)]
pub struct Tester {
    /// The processor which will be used for scheduling.
    pub processor: Arc<PatchProcessor>,
    /// The root directory for the artifacts.
    pub artifacts_root: PathBuf,
    /// The channel to which this results will be sent asynchronously.
    pub reports_sink: UnboundedSender<(Patch, io::Result<RunReport>)>,
}

impl Tester {
    /// Schedules a build-and-test process.
    /// # Arguments
    /// patch - solution to process.
    pub async fn schedule(self, patch: Patch) {
        let artifacts = self.artifacts_root.join(patch.id());

        task::spawn(async move {
            let res = if let Err(e) = prepare_dir(&artifacts).await {
                log::error!(
                    "Failed to prepare the artifacts directory at {} for solution {}. Error: {}.",
                    artifacts.display(),
                    patch.id(),
                    e
                );
                Err(e)
            } else {
                log::info!("Starting a test run for solution {}.", patch.id());
                self.processor.process(patch.path(), &artifacts).await
            };

            log::info!("Test run finished for solution {}.", patch.id());
            self.reports_sink.send((patch, res)).ok();
        });
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{patch_validator::PatchValidator, test_util::Env};
    use std::mem;
    use tokio::{sync::mpsc, time};

    #[ignore]
    #[tokio::test]
    async fn tester() {
        let env = Env::read();

        let mut validator = PatchValidator::default();
        let mut patches = vec![];
        for i in 0..3 {
            let name = format!("aa{}{}{}{}{}{}.patch", i, i, i, i, i, i);
            let path = env.base_path().join(name);
            fs::write(&path, format!("exit {}", i))
                .await
                .expect("failed to write file");
            let patch = validator
                .validate(&path)
                .await
                .expect("failed to validate patch");
            patches.push(patch);
        }

        let artifacts_root = env.base_path().join("artifacts");
        fs::create_dir(&artifacts_root)
            .await
            .expect("failed to make dir");

        let (tx, mut rx) = mpsc::unbounded_channel();

        let tester = Tester {
            processor: Arc::new(PatchProcessor {
                spawner: env.spawner(3),
                builder: env.builder(),
                base_image: env.base_image().path().into(),
                run_config: RunConfig {
                    execution: ExecutorConfig::test(),
                    build: Scenario {
                        retries: 0,
                        steps: vec![vec![Step::TransferPatch {
                            to: "patch".into(),
                            mode: 0o777,
                            timeout: Duration::from_secs(1),
                        }]],
                    },
                    tests: [(
                        "test".into(),
                        Scenario {
                            retries: 1,
                            steps: vec![vec![Step::Action {
                                action: SshAction::Exec {
                                    cmd: "./patch".into(),
                                },
                                timeout: Duration::from_secs(1),
                            }]],
                        },
                    )]
                    .into_iter()
                    .collect(),
                },
            }),
            artifacts_root,
            reports_sink: tx,
        };

        for patch in patches {
            tester.clone().schedule(patch).await;
        }

        mem::drop(tester);

        let reports = time::timeout(Duration::from_secs(180), async move {
            let mut reports = HashMap::new();
            while let Some((patch, result)) = rx.recv().await {
                reports.insert(patch.id().to_string(), result.expect("testing failed"));
            }
            reports
        })
        .await
        .expect("timeout");

        assert_eq!(reports.len(), 3);

        let report_0 = reports.get("aa000000").expect("missing report");
        let report_1 = reports.get("aa111111").expect("missing report");
        let report_2 = reports.get("aa222222").expect("missing report");

        for report in reports.values() {
            assert!(report.build().success());
            assert_eq!(report.tests().len(), 1);
            assert!(report.tests().contains_key("test"));
        }

        assert!(report_0.tests().get("test").unwrap().success());
        assert_eq!(report_0.tests().get("test").unwrap().attempts.len(), 1);

        assert!(!report_1.tests().get("test").unwrap().success());
        assert_eq!(report_1.tests().get("test").unwrap().attempts.len(), 2);

        assert!(!report_2.tests().get("test").unwrap().success());
        assert_eq!(report_2.tests().get("test").unwrap().attempts.len(), 2);
    }
}
