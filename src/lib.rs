use std::{
    io::{ErrorKind, Result},
    time::{Duration, Instant},
};

pub mod executor;
pub mod qemu;
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
