use std::{
    fmt::{self, Debug, Formatter},
    io::{self, ErrorKind},
    process,
    time::{Duration, Instant},
};

pub mod executor;
pub mod qemu;
pub mod ssh;

/// A struct for tracking a timeout between blocking function calls.
pub struct Timeout {
    start: Instant,
    duration: Duration,
}

impl Timeout {
    /// Creates a new instance of this struct.
    /// This struct will represent a timeout at `duration` from now.
    pub fn new(duration: Duration) -> Self {
        Self {
            start: Instant::now(),
            duration,
        }
    }

    /// Returns the [Duration] remaining to the timeout.
    /// If there is no time left, returns an [io::Error] of kind [ErrorKind::TimedOut].
    pub fn remaining(&self) -> io::Result<Duration> {
        let elapsed = self.start.elapsed();
        let remaining = self.duration.checked_sub(elapsed);

        match remaining {
            Some(r) if r > Duration::ZERO => Ok(r),
            _ => Err(ErrorKind::TimedOut.into()),
        }
    }

    /// Returns the number of milliseconds remaining to the timeout.
    /// If there is no time left, returns an [io::Error] of kind [ErrorKind::TimedOut].
    fn remaining_ms(&self) -> io::Result<u32> {
        let remaining = self.remaining()?.as_millis();
        if remaining > 0 {
            Ok(remaining.try_into().unwrap_or(u32::MAX))
        } else {
            Err(ErrorKind::TimedOut.into())
        }
    }
}

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

pub type Result<T> = core::result::Result<T, Error>;

/// An output of a successful command.
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
