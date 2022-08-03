use clap::Parser;
use qemu_test_runner::{
    config::Config,
    printer::Printer,
    qemu::{ImageBuilder, QemuConfig, QemuSpawner},
    tester::{PatchProcessor, RunConfig, RunReport, Tester},
    Error,
};
use std::{
    collections::HashSet,
    ffi::OsString,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
};
use tempfile::TempDir;
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWrite, BufReader},
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

enum MaybeTmp {
    Tmp(TempDir),
    NotTmp(PathBuf),
}

impl MaybeTmp {
    async fn at_path(path: PathBuf) -> Self {
        if let Err(e) = fs::create_dir_all(&path).await {
            if e.kind() != ErrorKind::AlreadyExists {
                panic!("failed to access directory {}: {}", path.display(), e);
            }
        }

        Self::NotTmp(path)
    }

    fn path(&self) -> &Path {
        match self {
            Self::Tmp(tmp) => tmp.path(),
            Self::NotTmp(path) => path.as_path(),
        }
    }
}

impl Default for MaybeTmp {
    fn default() -> Self {
        let dir = tempfile::tempdir().expect("failed to create a temporary directory");
        Self::Tmp(dir)
    }
}

struct TesterTask {
    tester: Tester,
    patch_source: UnboundedReceiver<PathBuf>,
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
    patch_sink: UnboundedSender<PathBuf>,
    seen_patches: HashSet<OsString>,
}

impl InputTask {
    fn new(patch_sink: UnboundedSender<PathBuf>) -> Self {
        Self {
            patch_sink,
            seen_patches: Default::default(),
        }
    }
    async fn run(mut self) -> io::Result<()> {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut buf = String::new();

        while reader.read_line(&mut buf).await? > 0 {
            let patch = PathBuf::from(&buf);
            buf.clear();

            let stem = match patch.file_stem() {
                Some(stem) if self.seen_patches.contains(stem) => {
                    eprintln!("patch {} already seen", stem.to_string_lossy());
                    continue;
                }
                Some(stem) => stem.to_os_string(),
                None => {
                    eprintln!("path {} does not have a stem", patch.display());
                    continue;
                }
            };

            self.seen_patches.insert(stem);

            if self.patch_sink.send(patch).is_err() {
                break;
            }
        }

        Ok(())
    }
}

async fn output_results<W>(
    mut printer: Printer<W>,
    mut results_rx: UnboundedReceiver<(PathBuf, Result<RunReport, Error>)>,
) where
    W: AsyncWrite,
{
    while let Some((patch, result)) = results_rx.recv().await {
        let err = printer.print(&patch, result.as_ref()).await.err();
        if let Some(err) = err {
            eprintln!(
                "failed to output results for patch {}: {}",
                patch.display(),
                err
            );
        }
    }
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

    let (report_tx, report_rx) = mpsc::unbounded_channel();
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

    let printer = Printer::new(reports.path().to_path_buf(), tokio::io::stdout());
    output_results(printer, report_rx).await;

    tester_task.await.expect("an internal task panicked");
    input_task
        .await
        .expect("an internal task panicked")
        .expect("an IO error occurred");
}
