use serde::{Serialize, Serializer};
use std::io;
pub mod config;
pub mod executor;
pub mod maybe_tmp;
pub mod patch_validator;
pub mod qemu;
pub mod ssh;
pub mod stats;
pub mod tasks;
pub mod tester;

#[derive(Serialize, Debug)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Output {
    Finished {
        exit_code: i32,
        #[serde(
            skip_serializing_if = "Vec::is_empty",
            serialize_with = "serialize_bytes_lossy"
        )]
        stdout: Vec<u8>,
        #[serde(
            skip_serializing_if = "Vec::is_empty",
            serialize_with = "serialize_bytes_lossy"
        )]
        stderr: Vec<u8>,
    },
    Timeout,
    Error {
        #[serde(serialize_with = "serialize_io_error")]
        error: io::Error,
    },
}

impl Output {
    pub fn success(&self) -> bool {
        matches!(self, Self::Finished { exit_code: 0, .. })
    }

    pub fn stdout(&self) -> Option<&[u8]> {
        match self {
            Self::Finished { stdout, .. } => Some(&stdout[..]),
            Self::Timeout => None,
            Self::Error { .. } => None,
        }
    }

    pub fn stderr(&self) -> Option<&[u8]> {
        match self {
            Self::Finished { stderr, .. } => Some(&stderr[..]),
            Self::Timeout => None,
            Self::Error { .. } => None,
        }
    }
}

fn serialize_io_error<S>(error: &io::Error, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.collect_str(error)
}

fn serialize_bytes_lossy<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let as_str = String::from_utf8_lossy(bytes);
    serializer.serialize_str(&as_str)
}

#[cfg(test)]
mod test_util {
    use crate::qemu::{Image, ImageBuilder, QemuConfig, QemuSpawner};
    use std::{
        env,
        ffi::OsString,
        path::{Path, PathBuf},
    };
    use tempfile::TempDir;

    pub struct Env {
        base_image: PathBuf,
        run_cmd: OsString,
        build_cmd: OsString,
        enable_kvm: bool,
        tmp: TempDir,
    }

    impl Env {
        const BASE_IMAGE_VAR: &'static str = "TEST_BASE_IMAGE";
        const RUN_CMD_VAR: &'static str = "TEST_RUN_CMD";
        const BUILD_CMD_VAR: &'static str = "TEST_BUILD_CMD";
        const ENABLE_KVM_VAR: &'static str = "TEST_ENABLE_KVM";

        fn assert_env(var: &str) -> OsString {
            env::var_os(var).unwrap_or_else(|| panic!("missing {} environment variable", var))
        }

        pub fn read() -> Self {
            let base_image = Self::assert_env(Self::BASE_IMAGE_VAR).into();
            let run_cmd = Self::assert_env(Self::RUN_CMD_VAR);
            let build_cmd = Self::assert_env(Self::BUILD_CMD_VAR);
            let enable_kvm = Self::assert_env(Self::ENABLE_KVM_VAR)
                .to_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| {
                    panic!(
                        "failed to parse the {} environment variable",
                        Self::ENABLE_KVM_VAR
                    )
                });

            let tmp = tempfile::tempdir().expect("failed to create a tmp directory");

            Self {
                base_image,
                run_cmd,
                build_cmd,
                enable_kvm,
                tmp,
            }
        }

        pub fn base_image(&self) -> Image<'_> {
            Image::Raw(self.base_image.as_path())
        }

        pub fn builder(&self) -> ImageBuilder {
            ImageBuilder {
                cmd: self.build_cmd.clone(),
            }
        }

        pub fn spawner(&self, concurrency: usize) -> QemuSpawner {
            QemuSpawner::new(
                concurrency,
                QemuConfig {
                    cmd: self.run_cmd.clone(),
                    memory: 1024,
                    enable_kvm: self.enable_kvm,
                    irqchip_off: true,
                },
            )
        }

        pub fn base_path(&self) -> &Path {
            self.tmp.path()
        }
    }
}
