use clap::Parser;
use futures::stream::StreamExt;
use qemu_test_runner::{
    maybe_tmp::MaybeTmp,
    patch_validator::{Patch, PatchValidator},
    prepare_dir,
    qemu::{ImageBuilder, QemuConfig, QemuSpawner},
    stats::Stats,
    tester::{PatchProcessor, RunConfig, RunReport},
};
use std::{
    ffi::OsString,
    io::{Error, ErrorKind, Result},
    path::PathBuf,
    process::ExitCode,
};
use tokio::{
    fs,
    io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader, Stdout},
    sync::Mutex,
};
use tokio_stream::wrappers::LinesStream;

#[derive(Parser, Debug)]
struct Args {
    #[clap(long)]
    /// Test suite configuration file.
    suite: PathBuf,
    #[clap(long, default_value_t = 1)]
    /// Maximal count of concurrent QEMU processes running.
    concurrency: usize,
    #[clap(long, default_value = "qemu-system-x86_64")]
    /// Command used to spawn new QEMU processes.
    qemu_system: OsString,
    #[clap(long, default_value_t = 1024)]
    /// Memory limit for a QEMU process (megabytes).
    qemu_memory: u16,
    #[clap(long, default_value_t = true)]
    /// Whether to enable KVM for QEMU processes.
    qemu_enable_kvm: bool,
    #[clap(long, default_value_t = true)]
    /// Whether to turn off the irqchip for QEMU processes.
    qemu_irqchip_off: bool,
    #[clap(long, default_value = "qemu-img")]
    /// Command used to create new qcow2 images.
    qemu_img: OsString,
    #[clap(long)]
    /// Base QEMU image (raw).
    base_image: PathBuf,
    #[clap(long)]
    /// Output directory for artifacts (qcow2 images).
    /// If omitted, artifacts will be saved in a temporary directory.
    artifacts: Option<PathBuf>,
    #[clap(long)]
    /// Output directory for detailed run reports.
    /// If omitted, reports will not be generated.
    reports: Option<PathBuf>,
}

async fn make_patch_processor(args: Args, artifacts_root: PathBuf) -> PatchProcessor {
    if args.concurrency == 0 {
        panic!("concurrency level cannot be set below 1");
    }

    let run_config = RunConfig::from_file(&args.suite)
        .await
        .expect("failed to process the suite file");

    let qemu_config = QemuConfig {
        cmd: args.qemu_system,
        memory: args.qemu_memory,
        enable_kvm: args.qemu_enable_kvm,
        irqchip_off: args.qemu_irqchip_off,
    };

    PatchProcessor {
        spawner: QemuSpawner::new(args.concurrency, qemu_config),
        builder: ImageBuilder { cmd: args.qemu_img },
        base_image: fs::canonicalize(args.base_image)
            .await
            .expect("failed to canonicalize the base image path"),
        run_config,
        artifacts_root,
    }
}

fn print_stats(stats: &Stats) {
    log::info!("{} solution(s) accepted.", stats.valid_solutions);
    log::info!("{} solution(s) rejected.", stats.invalid_solutions);

    if !stats.internal_errors.is_empty() {
        log::error!(
            "{} solution(s) not processed due to internal errors: {:?}.",
            stats.internal_errors.len(),
            stats.internal_errors,
        );
    }

    log::info!("{} solution(s) failed to build.", stats.builds_failed);

    let mut tests_with_failures = stats
        .test_failures
        .iter()
        .map(|(test, failures)| (test, *failures))
        .collect::<Vec<_>>();
    tests_with_failures.sort_unstable_by_key(|(_, failures)| *failures);
    log::info!("Tests by failures count: {:?}.", tests_with_failures);

    if !stats.missing_reports.is_empty() {
        log::error!(
            "Failed to save {} detailed reports for {:?}.",
            stats.missing_reports.len(),
            stats.missing_reports,
        );
    }
}

struct LineProcessor {
    patch_processor: PatchProcessor,
    patch_validator: Mutex<PatchValidator>,
    reports_dir: Option<PathBuf>,
    stats: Mutex<Stats>,
    stdout: Mutex<Stdout>,
}

impl LineProcessor {
    async fn print_results(&self, patch: &Patch, report: &RunReport) {
        let report_col = if report.build().success() {
            let failed_tests = report
                .tests()
                .iter()
                .filter(|(_, report)| !report.success())
                .map(|(name, _)| &name[..])
                .collect::<Vec<_>>();

            if failed_tests.is_empty() {
                "OK".into()
            } else {
                failed_tests.join(",")
            }
        } else {
            "build failed".into()
        };

        let line = format!("{};{}\n", patch, report_col);
        self.stdout
            .lock()
            .await
            .write_all(line.as_bytes())
            .await
            .expect("failed to write to stdout");
    }

    async fn save_report(&self, patch: &Patch, report: &RunReport) -> Result<()> {
        if let Some(dir) = self.reports_dir.as_ref() {
            let buf = serde_json::to_vec_pretty(report).map_err(|error| {
                Error::new(
                    ErrorKind::Other,
                    format!("failed to serialize report: {}", error),
                )
            })?;

            let mut path = dir.join(patch.id());
            path.set_extension("json");

            fs::write(&path, &buf[..]).await?;
            log::info!(
                "Successfuly saved report for solution {} at {}.",
                patch,
                path.display()
            );
        }

        Ok(())
    }

    async fn process(&self, line: String) {
        let patch = match self
            .patch_validator
            .lock()
            .await
            .validate(line.as_ref())
            .await
        {
            Ok(patch) => {
                log::info!("Starting to process solution {}.", patch);
                patch
            }
            Err(error) => {
                log::warn!("Invalid input line. Error: {}", error);
                self.stats.lock().await.solution_rejected();
                return;
            }
        };

        let run_result = self.patch_processor.process(&patch).await;
        self.stats.lock().await.patch_processed(&patch, &run_result);
        let report = match run_result {
            Ok(report) => {
                log::info!("Successfuly tested solution {}.", patch);
                report
            }
            Err(error) => {
                log::error!(
                    "An error occurred when testing solution {}: {}.",
                    patch,
                    error
                );
                return;
            }
        };

        self.print_results(&patch, &report).await;

        if let Err(error) = self.save_report(&patch, &report).await {
            log::error!(
                "An error occurred when saving the report for solution {}: {}.",
                patch,
                error
            );
            self.stats.lock().await.saving_report_failed(&patch);
        }
    }

    async fn run(self) -> Stats {
        LinesStream::new(BufReader::new(io::stdin()).lines())
            .map(|line| line.expect("failed to read to stdin"))
            .for_each_concurrent(None, |line| self.process(line))
            .await;

        self.stats.into_inner()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    env_logger::init();

    let args = Args::parse();
    log::debug!("Program is running with args: {:?}.", args);

    let (artifacts, reports_dir) = {
        let artifacts = match args.artifacts.as_ref() {
            Some(path) => MaybeTmp::at_path(path.to_path_buf())
                .await
                .expect("failed to access the artifacts directory"),
            None => {
                let tmp = MaybeTmp::tmp().expect("failed to create a temporary directory");
                log::info!("Artifacts direcrory was not specified, artifacts will not be saved.",);
                tmp
            }
        };
        let reports_dir = args.reports.clone();
        match reports_dir.as_ref() {
            Some(dir) => prepare_dir(dir.as_path())
                .await
                .expect("failed to access the reports directory"),
            None => log::info!("Reports directory was not specified, reports will not be saved."),
        }

        (artifacts, reports_dir)
    };

    let lines_processor = LineProcessor {
        patch_processor: make_patch_processor(args, artifacts.path().to_path_buf()).await,
        patch_validator: Default::default(),
        reports_dir,
        stats: Default::default(),
        stdout: Mutex::new(io::stdout()),
    };

    let stats = lines_processor.run().await;
    print_stats(&stats);

    if stats.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
