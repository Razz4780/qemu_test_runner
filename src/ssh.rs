use ssh2::Session;
use std::{
    fs::File,
    io::{self, ErrorKind, Read, Result},
    net::{SocketAddr, TcpStream},
    os::unix::prelude::ExitStatusExt,
    path::Path,
    process::{ExitStatus, Output},
    time::{Duration, Instant},
};

struct Timeout {
    start: Instant,
    duration: Duration,
}

impl Timeout {
    fn new(duration: Duration) -> Self {
        Self {
            start: Instant::now(),
            duration,
        }
    }

    fn remaining(&self) -> Result<Duration> {
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

pub struct Controller {
    session: Session,
}

impl Controller {
    pub fn new(
        addr: SocketAddr,
        username: &str,
        password: &str,
        timeout: Duration,
    ) -> Result<Self> {
        let timeout = Timeout::new(timeout);

        let conn = TcpStream::connect_timeout(&addr, timeout.remaining()?)?;

        let mut session = Session::new()?;
        session.set_tcp_stream(conn);
        session.set_timeout(timeout.remaining_ms()?);
        session.handshake()?;
        session.set_timeout(timeout.remaining_ms()?);
        session.userauth_password(username, password)?;

        Ok(Self { session })
    }

    pub fn exec(&mut self, cmd: &str, timeout: Duration) -> Result<Output> {
        let timeout = Timeout::new(timeout);

        self.session.set_timeout(timeout.remaining_ms()?);
        let mut channel = self.session.channel_session()?;
        self.session.set_timeout(timeout.remaining_ms()?);
        channel.exec(cmd)?;

        let mut stdout = Vec::new();
        self.session.set_timeout(timeout.remaining_ms()?);
        channel.read_to_end(&mut stdout)?;

        let mut stderr = Vec::new();
        self.session.set_timeout(timeout.remaining_ms()?);
        channel.stderr().read_to_end(&mut stderr)?;

        self.session.set_timeout(timeout.remaining_ms()?);
        channel.wait_close()?;
        self.session.set_timeout(timeout.remaining_ms()?);
        let exit_status = channel.exit_status()?;

        Ok(Output {
            status: ExitStatus::from_raw(exit_status),
            stdout,
            stderr,
        })
    }

    pub fn send<P1, P2>(
        &mut self,
        local: P1,
        remote: P2,
        mode: i32,
        timeout: Duration,
    ) -> Result<()>
    where
        P1: AsRef<Path>,
        P2: AsRef<Path>,
    {
        let timeout = Timeout::new(timeout);

        let mut file = File::open(local.as_ref())?;
        let size = file.metadata()?.len();

        self.session.set_timeout(timeout.remaining_ms()?);
        let mut remote_file = self.session.scp_send(remote.as_ref(), mode, size, None)?;
        self.session.set_timeout(timeout.remaining_ms()?);
        io::copy(&mut file, &mut remote_file)?;

        self.session.set_timeout(timeout.remaining_ms()?);
        remote_file.send_eof()?;
        self.session.set_timeout(timeout.remaining_ms()?);
        remote_file.wait_eof()?;
        self.session.set_timeout(timeout.remaining_ms()?);
        remote_file.close()?;
        self.session.set_timeout(timeout.remaining_ms()?);
        remote_file.wait_close()?;

        Ok(())
    }
}
