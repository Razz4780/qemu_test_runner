use qemu_test_runner::qemu::{Image, ImageBuilder, QemuConfig, QemuSpawner};
use std::env;

#[tokio::main]
async fn main() {
    let raw_minix_img =
        env::var_os("RAW_MINIX_IMG").expect("expected path to base MINIX image in RAW_MINIX_IMG");

    let tmp = tempfile::tempdir().unwrap();

    let base = tmp.path().join("base.img");
    let test_1 = tmp.path().join("test_1.img");
    let test_2 = tmp.path().join("test_2.img");

    let builder = ImageBuilder {
        cmd: "qemu-img".into(),
    };

    builder
        .create(Image::Raw(raw_minix_img.as_ref()), Image::Qcow2(&base))
        .await
        .unwrap();

    tokio::try_join!(
        builder.create(Image::Qcow2(&base), Image::Qcow2(&test_1)),
        builder.create(Image::Qcow2(&base), Image::Qcow2(&test_2)),
    )
    .unwrap();

    let spawner = QemuSpawner::new(
        2,
        QemuConfig {
            cmd: "qemu-system-x86_64".into(),
            memory: 1024,
            enable_kvm: true,
            irqchip_off: true,
        },
    );

    let (mut instance_1, mut instance_2) = tokio::try_join!(
        spawner.spawn(test_1.into_os_string()),
        spawner.spawn(test_2.into_os_string())
    )
    .unwrap();

    let (ssh1, ssh2) = tokio::try_join!(instance_1.ssh(), instance_2.ssh()).unwrap();

    println!("{} {}", ssh1, ssh2);

    tokio::try_join!(instance_1.kill(), instance_2.kill()).unwrap();
}
