use clap::Parser;
use qemu_test_runner::{
    config::Config,
    qemu::{ImageBuilder, QemuConfig, QemuSpawner},
    tester::{TestReport, Tester},
};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::task;

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
    #[clap(long, value_parser, default_value = ".")]
    /// Output directory for results.
    results: PathBuf,
}

fn make_tester(args: Args) -> Tester {
    let config: Config = {
        let bytes = fs::read(&args.suite).expect("failed to read the suite file");
        serde_json::from_slice(&bytes[..]).expect("failed to parse the suite file")
    };

    let mut tests = HashMap::new();
    for (suite_name, suite) in &config.test_suites {
        for (test_name, test) in &suite.tests {
            tests.insert(
                format!("suite_{}_test_{}", suite_name, test_name),
                test.config.clone(),
            );
        }
    }

    Tester {
        spawner: QemuSpawner::new(
            args.concurrency,
            QemuConfig {
                cmd: args.qemu_system,
                memory: args.qemu_memory,
                enable_kvm: args.qemu_enable_kvm,
                irqchip_off: args.qemu_irqchip_off,
            },
        ),
        builder: ImageBuilder { cmd: args.qemu_img },
        base_image: args.minix_base,
        build_config: config.build,
        patch_dst: config.patch_dst,
        execution_config: config.execution,
        tests,
    }
}

fn output_results(_dst: &Path, _results: &[TestReport]) {
    todo!()
}

fn read_patches() -> Vec<PathBuf> {
    let mut stems = HashSet::new();
    io::stdin()
        .lines()
        .map(|l| {
            let path: &Path = l.as_ref().expect("failed to read from stdin").as_ref();
            path.canonicalize().expect("failed to canonicalize path")
        })
        .filter(|patch| {
            if let Some(stem) = patch.file_stem() {
                stems.insert(stem.to_os_string())
            } else {
                false
            }
        })
        .collect::<Vec<_>>()
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let (_tmp, artifacts) = if let Some(path) = args.artifacts.clone() {
        (None, path)
    } else {
        let tmp = tempfile::tempdir().expect("failed to create a temporary directory");
        let path = tmp.path().to_path_buf();
        (Some(tmp), path)
    };
    let results_dst = args.results.clone();

    let tester = Arc::new(make_tester(args));

    let patches = read_patches();

    let mut handles = Vec::with_capacity(patches.len());
    for patch in patches {
        let tester = tester.clone();
        let artifacts = artifacts.join(patch.file_stem().unwrap());
        let handle = task::spawn(async move { tester.process(&patch, artifacts.into()).await });
        handles.push(handle);
    }

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await {
            Ok(Ok(report)) => results.push(report),
            Ok(Err(_)) => {}
            Err(_) => {}
        }
    }

    output_results(&results_dst, &results[..]);
}
