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

#[derive(Default, Serialize)]
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

    pub fn success(&self) -> bool {
        self.attempts
            .last()
            .map(|reports| reports.iter().all(ExecutorReport::success))
            .unwrap_or(true)
    }
}

#[derive(Serialize)]
pub struct RunReport {
    build: ScenarioReport,
    tests: HashMap<String, ScenarioReport>,
}

impl RunReport {
    pub fn build(&self) -> &ScenarioReport {
        &self.build
    }

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

    pub async fn process(
        self: Arc<Self>,
        patch: &Path,
        artifacts_root: &Path,
    ) -> io::Result<RunReport> {
        let mut res = RunReport {
            build: self.build(patch, artifacts_root).await?,
            tests: Default::default(),
        };
        if !res.build.success() {
            return Ok(res);
        }

        let mut handles = self
            .spawn_test_workers(patch, artifacts_root, res.build.last_image())
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

                    return Err(io::Error::new(io::ErrorKind::Other, error));
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
    pub reports_sink: UnboundedSender<(Patch, io::Result<RunReport>)>,
}

impl Tester {
    pub async fn schedule(self, patch: Patch) {
        let artifacts = self.artifacts_root.join(patch.id());

        task::spawn(async move {
            let res = if let Err(e) = prepare_dir(&artifacts).await {
                Err(e)
            } else {
                self.processor.process(patch.path(), &artifacts).await
            };

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
