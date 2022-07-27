use std::{
    io::{self, ErrorKind},
    os::unix::process::ExitStatusExt,
    process::Output,
    time::{Duration, Instant},
};

pub mod executor;
pub mod qemu;
pub mod runner;
pub mod ssh;

pub struct Timeout {
    start: Instant,
    duration: Duration,
}

impl Timeout {
    pub fn new(duration: Duration) -> Self {
        Self {
            start: Instant::now(),
            duration,
        }
    }

    pub fn remaining(&self) -> io::Result<Duration> {
        let elapsed = self.start.elapsed();
        let remaining = self.duration.checked_sub(elapsed);

        match remaining {
            Some(r) if r > Duration::ZERO => Ok(r),
            _ => Err(ErrorKind::TimedOut.into()),
        }
    }

    fn remaining_ms(&self) -> io::Result<u32> {
        let remaining = self.remaining()?.as_millis();
        if remaining > 0 {
            Ok(remaining.try_into().unwrap_or(u32::MAX))
        } else {
            Err(ErrorKind::TimedOut.into())
        }
    }
}

#[derive(Debug)]
pub struct Error {
    pub error: io::Error,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self {
            error,
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

pub trait CanFail: Sized {
    fn result(self) -> Result<Self>;
}

impl CanFail for Output {
    fn result(self) -> Result<Self> {
        if self.status.success() {
            Ok(self)
        } else {
            let error = io::Error::from_raw_os_error(self.status.into_raw());
            Err(Error {
                error,
                stdout: self.stdout,
                stderr: self.stderr,
            })
        }
    }
}
