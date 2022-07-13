use std::{
    io::{ErrorKind, Result},
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

    pub fn remaining(&self) -> Result<Duration> {
        let elapsed = self.start.elapsed();
        let remaining = self.duration.checked_sub(elapsed);

        match remaining {
            Some(r) if r > Duration::ZERO => Ok(r),
            _ => Err(ErrorKind::TimedOut.into()),
        }
    }

    fn remaining_ms(&self) -> Result<u32> {
        let remaining = self.remaining()?.as_millis();
        if remaining > 0 {
            Ok(remaining.try_into().unwrap_or(u32::MAX))
        } else {
            Err(ErrorKind::TimedOut.into())
        }
    }
}

pub trait CanFail {
    fn failed(&self) -> bool;
}

impl CanFail for () {
    fn failed(&self) -> bool {
        false
    }
}

impl CanFail for Output {
    fn failed(&self) -> bool {
        !self.status.success()
    }
}

impl<C> CanFail for Result<C>
where
    C: CanFail,
{
    fn failed(&self) -> bool {
        self.as_ref().map(|res| res.failed()).unwrap_or(true)
    }
}

impl<C> CanFail for Vec<C>
where
    C: CanFail,
{
    fn failed(&self) -> bool {
        self.last().map(CanFail::failed).unwrap_or(false)
    }
}
