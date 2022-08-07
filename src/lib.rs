use serde::{ser::SerializeStruct, Serialize, Serializer};
use std::{
    fmt::{self, Debug, Display, Formatter},
    io, process,
};
use tokio::time::error::Elapsed;

pub mod config;
pub mod executor;
pub mod maybe_tmp;
pub mod patch_validator;
pub mod qemu;
pub mod ssh;
pub mod stats;
pub mod tasks;
pub mod tester;

/// An error that can occurr when executing a command.
#[derive(Debug)]
pub struct Error {
    /// An empty error probably means that the child process was killed by a signal.
    pub error: Option<io::Error>,
    pub stdout: String,
    pub stderr: String,
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.error {
            Some(error) => write!(f, "{}", error),
            None => f.write_str("process was killed by a signal"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self {
            error: Some(error),
            stdout: Default::default(),
            stderr: Default::default(),
        }
    }
}

impl From<ssh2::Error> for Error {
    fn from(error: ssh2::Error) -> Self {
        io::Error::from(error).into()
    }
}

impl From<Elapsed> for Error {
    fn from(_: Elapsed) -> Self {
        Self {
            error: Some(io::ErrorKind::TimedOut.into()),
            stdout: Default::default(),
            stderr: Default::default(),
        }
    }
}

impl<'a> From<&'a mut Error> for &'a Error {
    fn from(error: &'a mut Error) -> Self {
        error
    }
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("Error", 3)?;
        s.serialize_field("error", &format!("{}", self))?;
        s.serialize_field("stdout", &self.stdout)?;
        s.serialize_field("stderr", &self.stderr)?;
        s.end()
    }
}

/// An output of a successful command.
#[derive(Default, Debug, Serialize)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

impl TryFrom<process::Output> for Output {
    type Error = Error;

    fn try_from(output: process::Output) -> Result<Self, Self::Error> {
        if output.status.success() {
            Ok(Self {
                stdout: String::from_utf8_lossy(&output.stdout[..]).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr[..]).into_owned(),
            })
        } else {
            let error = output.status.code().map(io::Error::from_raw_os_error);
            Err(Error {
                error,
                stdout: String::from_utf8_lossy(&output.stdout[..]).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr[..]).into_owned(),
            })
        }
    }
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
