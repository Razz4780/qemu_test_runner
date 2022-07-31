use clap::Parser;
use std::{ffi::OsString, path::PathBuf};

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

#[tokio::main]
async fn main() {
    let args = Args::parse();

    println!("{:?}", args);
}
