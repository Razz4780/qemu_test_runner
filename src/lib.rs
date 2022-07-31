use std::{
    fmt::{self, Debug, Formatter},
    io, process,
};
use tokio::time::error::Elapsed;

pub mod executor;
pub mod qemu;
pub mod ssh;
pub mod tester;

/// An error that can occurr when executing a command.
pub struct Error {
    /// An empty error probably means that the child process was killed by a signal.
    pub error: Option<io::Error>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Debug for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("error", &self.error)
            .field("stdout", &String::from_utf8_lossy(&self.stdout[..]))
            .field("stderr", &String::from_utf8_lossy(&self.stderr[..]))
            .finish()
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

pub type Result<T> = core::result::Result<T, Error>;

/// An output of a successful command.
#[derive(Default)]
pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Debug for Output {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Output")
            .field("stdout", &String::from_utf8_lossy(&self.stdout[..]))
            .field("stderr", &String::from_utf8_lossy(&self.stderr[..]))
            .finish()
    }
}

impl TryFrom<process::Output> for Output {
    type Error = Error;

    fn try_from(output: process::Output) -> Result<Self> {
        if output.status.success() {
            Ok(Self {
                stdout: output.stdout,
                stderr: output.stderr,
            })
        } else {
            let error = output.status.code().map(io::Error::from_raw_os_error);
            Err(Error {
                error,
                stdout: output.stdout,
                stderr: output.stderr,
            })
        }
    }
}
