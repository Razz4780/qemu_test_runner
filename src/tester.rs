use crate::{
    executor::{stack::StackExecutor, ExecutorConfig, ExecutorReport},
    patch_validator::Patch,
    prepare_dir,
    qemu::{Image, ImageBuilder, QemuSpawner},
    ssh::SshAction,
};
use futures::{stream::FuturesUnordered, StreamExt};
use serde::Serialize;
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    time::Duration,
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
        /// Timeout for this transfer.
        timeout: Duration,
    },
}

impl Step {
    fn action(&self, patch: &Path) -> SshAction {
        match self {
            Self::Action { action, .. } => action.clone(),
            Self::TransferPatch { to, .. } => SshAction::Send {
                from: patch.to_path_buf(),
                to: to.clone(),
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
pub struct ScenarioReport(Vec<Vec<ExecutorReport>>);

impl ScenarioReport {
    fn push_attempt(&mut self, attempt: Vec<ExecutorReport>) {
        self.0.push(attempt);
    }

    fn last_image(&self) -> Option<&Path> {
        let image = self.0.last()?.last()?.image();

        Some(image)
    }

    /// # Returns
    /// Whether the scenario was successful.
    pub fn success(&self) -> bool {
        self.0
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
    /// Root directory for artifacts.
    pub artifacts_root: PathBuf,
}

impl PatchProcessor {
    async fn run_scenario(
        &self,
        patch: &Patch,
        base_image: Image<'_>,
        artifacts: &Path,
        scenario: &Scenario,
        name: &str,
    ) -> io::Result<ScenarioReport> {
        let mut report = ScenarioReport::default();

        for i in 0..=scenario.retries {
            log::info!(
                "Initializing attempt {} of scenario {} for solution {}.",
                i + 1,
                name,
                patch
            );

            let dst = artifacts.join(format!("attempt_{}.qcow2", i + 1));
            self.builder
                .create(base_image, Image::Qcow2(dst.as_ref()))
                .await?;

            let mut executor =
                StackExecutor::new(&self.run_config.execution, &self.spawner, dst.as_os_str());

            for phase in &scenario.steps {
                let iter = phase
                    .iter()
                    .map(|step| (step.action(patch.path()), step.timeout()));

                let success = executor.open_stack().await?.run_until_failure(iter).await?;
                if !success {
                    log::info!(
                        "Attempt {} of scenario {} failed for solution {}.",
                        i + 1,
                        name,
                        patch
                    );
                    break;
                }
            }

            let attempt = executor.finish();
            report.push_attempt(attempt);

            if report.success() {
                break;
            }
        }

        Ok(report)
    }

    /// Executes the build-and-test process for a single [Patch].
    /// # Arguments
    /// patch - the solution to process.
    /// # Returns
    /// A [RunReport] from the process.
    pub async fn process(&self, patch: &Patch) -> io::Result<RunReport> {
        let root = self.artifacts_root.join(patch.id());
        prepare_dir(root.as_path()).await?;

        log::info!("Building a test image for solution {}.", patch);
        let build_root = root.join("build");
        prepare_dir(build_root.as_path()).await?;

        let build = self
            .run_scenario(
                patch,
                Image::Raw(self.base_image.as_path()),
                build_root.as_path(),
                &self.run_config.build,
                "build",
            )
            .await?;

        let tests = if build.success() {
            log::info!("Running tests for solution {}.", patch);
            let tests_root = root.join("tests");
            prepare_dir(tests_root.as_path()).await?;

            let test_image = build
                .last_image()
                .map(Image::Qcow2)
                .unwrap_or(Image::Raw(self.base_image.as_path()));

            let mut futs = FuturesUnordered::new();
            for (test, scenario) in &self.run_config.tests {
                let test_root = tests_root.join(test);
                futs.push(async move {
                    prepare_dir(test_root.as_path()).await?;
                    let report = self
                        .run_scenario(patch, test_image, test_root.as_path(), scenario, test)
                        .await?;
                    Ok::<_, io::Error>((test.clone(), report))
                });
            }

            let mut tests = HashMap::new();
            while let Some(result) = futs.next().await {
                match result {
                    Ok((test, report)) => {
                        log::info!("Received report from test {} for solution {}.", test, patch);
                        tests.insert(test.clone(), report);
                    }
                    Err(error) => {
                        log::error!(
                            "An unexpected error occurred when running tests for solution {}. Error: {}.",
                            patch,
                            error
                        );
                        return Err(error);
                    }
                }
            }

            tests
        } else {
            log::info!("Build process failed for solution {}.", patch);

            Default::default()
        };

        Ok(RunReport { build, tests })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{patch_validator::PatchValidator, test_util::Env};
    use tokio::{fs, time};

    #[ignore]
    #[tokio::test]
    async fn concurrent_tests() {
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

        let processor = PatchProcessor {
            spawner: env.spawner(3),
            builder: env.builder(),
            base_image: env.base_image().path().into(),
            run_config: RunConfig {
                execution: ExecutorConfig::test(),
                build: Scenario {
                    retries: 0,
                    steps: vec![vec![Step::TransferPatch {
                        to: "patch".into(),
                        timeout: Duration::from_secs(1),
                    }]],
                },
                tests: HashMap::from([(
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
                )]),
            },
            artifacts_root: env.base_path().join("artifacts"),
        };

        let proc = &processor;
        let futs = FuturesUnordered::new();
        for patch in &patches {
            futs.push(async move {
                let report = proc.process(patch).await.expect("testing failed");
                (patch.id(), report)
            });
        }
        let reports = time::timeout(Duration::from_secs(180), futs.collect::<HashMap<_, _>>())
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
        assert_eq!(report_0.tests().get("test").unwrap().0.len(), 1);

        assert!(!report_1.tests().get("test").unwrap().success());
        assert_eq!(report_1.tests().get("test").unwrap().0.len(), 2);

        assert!(!report_2.tests().get("test").unwrap().success());
        assert_eq!(report_2.tests().get("test").unwrap().0.len(), 2);
    }
}
