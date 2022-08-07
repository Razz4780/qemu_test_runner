use clap::Parser;
use qemu_test_runner::{
    config::Config,
    maybe_tmp::MaybeTmp,
    patch_validator::Patch,
    qemu::{ImageBuilder, QemuConfig, QemuSpawner},
    stats::Stats,
    tasks::{InputTask, TesterTask},
    tester::{PatchProcessor, RunConfig, RunReport, Tester},
    Error,
};
use std::{
    ffi::OsString,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{fs, sync::mpsc, task};

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
        let config = Config::from_file(&args.suite)
            .await
            .expect("failed to process the config file");
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
        base_image: fs::canonicalize(args.minix_base)
            .await
            .expect("failed to canonicalize the base image path"),
        run_config,
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

    let artifacts = match args.artifacts.as_ref() {
        Some(path) => MaybeTmp::at_path(path)
            .await
            .expect("failed to access the artifacts directory"),
        None => MaybeTmp::tmp().expect("failed to create a temporary directory"),
    };
    let reports = args.reports.clone();

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
    let input_task = task::spawn(async move {
        InputTask::new(patch_tx)
            .run()
            .await
            .expect("an IO error occurred")
    });

    let mut stats = Stats::default();
    while let Some((patch, result)) = report_rx.recv().await {
        stats.update(patch.path(), result.as_ref());
        print_result(&patch, result.as_ref());

        if let (Ok(report), Some(path)) = (result, reports.as_ref()) {
            if let Err(error) = save_report(path, &patch, &report).await {
                eprintln!(
                    "Failed to save the report for the patch {}, error: {:?}",
                    patch.path().display(),
                    error
                );
            }
        }
    }

    if let Err(e) = tokio::try_join!(tester_task, input_task) {
        eprintln!("An internal task panicked with error: {}", e);
        eprintln!("Finishind early");
    } else {
        eprintln!("Finished");
    }

    eprintln!(
        "{} solution(s) processed successfuly",
        stats.solutions() - stats.internal_errors().len()
    );
    if !stats.internal_errors().is_empty() {
        eprintln!(
            "{} solution(s) not processed due to internal errors",
            stats.internal_errors().len()
        );
        for path in stats.internal_errors() {
            eprintln!("  - {}", path.display());
        }
    }
    eprintln!("{} solution(s) did not build", stats.builds_failed());
    if !stats.test_failures().is_empty() {
        eprintln!("Tests by failures count:");
        let mut tests_with_failures = stats
            .test_failures()
            .iter()
            .map(|(test, failures)| (&test[..], *failures))
            .collect::<Vec<_>>();
        tests_with_failures.sort_unstable_by_key(|(_, failures)| *failures);
        for (test, failures) in tests_with_failures.into_iter().rev() {
            eprintln!("  - {}: {}", test, failures);
        }
    }
    if let Some(path) = reports {
        eprintln!("Detailed reports saved in {}", path.display());
    }
}
