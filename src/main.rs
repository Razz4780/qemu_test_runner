// use qemu_test_runner::{
//     executor::{Action, Config, Executor},
//     qemu::{Build, Forward, Run},
// };
// use std::{
//     io::Result,
//     net::{Ipv4Addr, SocketAddr},
//     time::Duration,
// };

// fn main() -> Result<()> {
//     let qcow = "../SO/minix_qcow2.img";
//     Build::default().qcow2("../SO/MINIX".as_ref(), qcow.as_ref())?;
//     let forward = Forward::new_ssh().unwrap();
//     let port = forward.from;
//     let instance = Run::default().forward(forward).spawn(qcow.as_ref())?;
//     let executor = Executor::new(
//         instance,
//         Config {
//             ssh_addr: SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port),
//             ssh_username: "root".into(),
//             ssh_password: "root".into(),
//             startup_timeout: Duration::from_secs(20),
//             poweroff_timeout: Duration::from_secs(10),
//             poweroff_cmd: "/sbin/poweroff".into(),
//         },
//     );
//     let report = executor.run(vec![
//         Action::Send {
//             local: "./Cargo.toml".into(),
//             remote: "/root/Cargo.toml".into(),
//             mode: 0o777,
//             timeout: Duration::from_secs(1),
//         },
//         Action::Exec {
//             cmd: "ls /root".into(),
//             timeout: Duration::from_secs(1),
//         },
//         Action::Exec {
//             cmd: "ls /chujXD".into(),
//             timeout: Duration::from_secs(1),
//         },
//         Action::Exec {
//             cmd: "ls /root".into(),
//             timeout: Duration::from_secs(1),
//         },
//     ]);

//     println!("{:?}", report);

//     Ok(())
// }

#[tokio::main]
async fn main() {
    println!("{:?}", std::io::Error::from_raw_os_error(0));
}
