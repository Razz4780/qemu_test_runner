// use crate::{
//     executor::{Action, ExecutionConfig, ExecutionReport, Executor},
//     qemu::{Build, Instance, InstanceSpawner, Run},
//     CanFail, Timeout,
// };
// use std::{
//     cmp,
//     ffi::{OsStr, OsString},
//     fs,
//     io::Result,
//     net::{Ipv4Addr, SocketAddr},
//     path::{Path, PathBuf},
//     thread,
//     time::Duration,
// };
// use tokio::process::Command;

// pub struct Runner {
//     spawner: InstanceSpawner,
//     qemu_run: Run,
//     steps: Vec<ExecutionConfig>,
// }

// impl Runner {
//     async fn execute_on(&self, image: OsString, config: ExecutionConfig) -> Result<ExecutionReport> {
//         let instance = self.spawner.spawn(&self.qemu_run, image).await?;

//         let executor = Executor::new(instance, config);

//         Ok(executor.run().await)
//     }

//     pub async fn run_on(
//         &self,
//         image: &OsStr,
//     ) -> Vec<Result<ExecutionReport>> {
//         let mut reports = Vec::with_capacity(self.steps.len());

//         for step in &self.steps {
//             let res = self.execute_on(image, step).await;
//             reports.push(res);

//             if reports.failed() {
//                 break;
//             }
//         }

//         reports
//     }
// }
