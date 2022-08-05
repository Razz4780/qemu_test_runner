use clap::Parser;
use qemu_test_runner::{
    config::Config,
    maybe_tmp::MaybeTmp,
    patch_validator::{Patch, PatchValidator},
    qemu::{ImageBuilder, QemuConfig, QemuSpawner},
    tester::{PatchProcessor, RunConfig, RunReport, Tester},
    Error,
};
use std::{
    ffi::OsString,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    fs,
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task,
};

#[derive(Parser, Debug)]
struct Args {
    #[clap(long, value_parser)]
    /// Test suite configuration file.
    suite: PathBuf,
    #[clap(long, value_parser, default_value_t = 1)]
    /// Maximal count of concurrent QEMU processes running.
    concurrency: usize,
    #[clap(long, value_parser, default_value = "qemu-system-x86_64")]
    /// Command used to invoke a QEMU process.
    qemu_system: OsString,
    #[clap(long, value_parser, default_value_t = 1024)]
    /// Memory limit for a QEMU process (megabytes).
    qemu_memory: u16,
    #[clap(long, value_parser, default_value_t = true)]
    /// Whether to enable KVM for QEMU processes.
    qemu_enable_kvm: bool,
    #[clap(long, value_parser, default_value_t = true)]
    /// Whether to turn off the irqchip for QEMU processes.
    qemu_irqchip_off: bool,
    #[clap(long, value_parser, default_value = "qemu-img")]
    /// Command used to work with QEMU images.
    qemu_img: OsString,
    #[clap(long, value_parser)]
    /// Base MINIX3 image.
    minix_base: PathBuf,
    #[clap(long, value_parser)]
    /// Output directory for artifacts (qcow2 images).
    artifacts: Option<PathBuf>,
    #[clap(long, value_parser)]
    /// Output directory for detailed run reports.
    reports: Option<PathBuf>,
}

async fn make_patch_processor(args: Args) -> PatchProcessor {
    let run_config: RunConfig = {
        let bytes = fs::read(&args.suite)
            .await
            .expect("failed to read the suite file");
        let config: Config =
            serde_yaml::from_slice(&bytes[..]).expect("failed to parse the suite file");
        config.try_into().expect("invalid suite configuration")
    };

    let qemu_config = QemuConfig {
        cmd: args.qemu_system,
        memory: args.qemu_memory,
        enable_kvm: args.qemu_enable_kvm,
        irqchip_off: args.qemu_irqchip_off,
    };

    PatchProcessor {
        spawner: QemuSpawner::new(args.concurrency, qemu_config),
        builder: ImageBuilder { cmd: args.qemu_img },
        base_image: args.minix_base,
        run_config,
    }
}

struct TesterTask {
    tester: Tester,
    patch_source: UnboundedReceiver<Patch>,
}

impl TesterTask {
    async fn run(mut self) {
        while let Some(patch) = self.patch_source.recv().await {
            if let Err(e) = self.tester.clone().schedule(patch).await {
                eprintln!("an error occurred: {}", e);
            }
        }
    }
}

struct InputTask {
    patch_sink: UnboundedSender<Patch>,
    validator: PatchValidator,
}

impl InputTask {
    fn new(patch_sink: UnboundedSender<Patch>) -> Self {
        Self {
            patch_sink,
            validator: Default::default(),
        }
    }
    async fn run(mut self) -> io::Result<()> {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut buf = String::new();

        while reader.read_line(&mut buf).await? > 0 {
            let path = PathBuf::from(&buf);
            buf.clear();

            let patch = match self.validator.validate(path.as_path()).await {
                Ok(patch) => patch,
                Err(error) => {
                    eprintln!("Invalid path {}: {}", path.display(), error);
                    continue;
                }
            };

            if self.patch_sink.send(patch).is_err() {
                break;
            }
        }

        Ok(())
    }
}

fn print_result(patch: &Patch, result: Result<&RunReport, &Error>) {
    let result_col = match result {
        Ok(report) if report.build().err().is_some() => "build failed".into(),
        Ok(report) => {
            let failed_tests = report
                .tests()
                .iter()
                .filter_map(|(name, report)| report.err().is_some().then_some(&name[..]))
                .collect::<Vec<_>>()
                .join(",");
            if failed_tests.is_empty() {
                "OK".into()
            } else {
                failed_tests
            }
        }
        Err(error) => format!("error during testing: {}", error),
    };

    println!("{};{};{}", patch.id(), patch.path().display(), result_col);
}

async fn save_report(reports_dir: &Path, patch: &Patch, report: &RunReport) -> io::Result<()> {
    let mut buf = Vec::with_capacity(4096);
    serde_yaml::to_writer(&mut buf, report).map_err(|e| io::Error::new(ErrorKind::Other, e))?;

    let mut path = reports_dir.join(patch.id());
    path.set_extension("yaml");
    fs::write(path, buf).await
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let artifacts = match args.artifacts.clone() {
        Some(path) => MaybeTmp::at_path(path).await,
        None => MaybeTmp::default(),
    };
    let reports = match args.reports.clone() {
        Some(path) => MaybeTmp::at_path(path).await,
        None => MaybeTmp::default(),
    };

    let (report_tx, mut report_rx) = mpsc::unbounded_channel();
    let (patch_tx, patch_rx) = mpsc::unbounded_channel();

    let tester_task = {
        let task = TesterTask {
            tester: Tester {
                processor: Arc::new(make_patch_processor(args).await),
                artifacts_root: artifacts.path().to_path_buf(),
                reports_sink: report_tx,
            },
            patch_source: patch_rx,
        };
        task::spawn(task.run())
    };
    let input_task = task::spawn(InputTask::new(patch_tx).run());

    let mut total = 0;
    let mut failed = 0;
    while let Some((patch, result)) = report_rx.recv().await {
        total += 1;
        if result.is_err() {
            failed += 1;
        }

        print_result(&patch, result.as_ref());
        if let Ok(report) = result {
            if let Err(error) = save_report(reports.path(), &patch, &report).await {
                eprintln!(
                    "Failed to save the test for the patch {}, error: {:?}",
                    patch.path().display(),
                    error
                );
            }
        }
    }

    tester_task.await.expect("an internal task panicked");
    input_task
        .await
        .expect("an internal task panicked")
        .expect("an IO error occurred");

    eprintln!("Finished");
    eprintln!("{} solution(s) processed", total);
    if failed > 0 {
        eprintln!("Processing {} solution(s) failed", failed);
    }
    eprintln!("Detailed reports saved in {}", reports.path().display());
}
