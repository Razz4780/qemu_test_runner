use ssh2::Session;
use std::{
    fs::File,
    io::{self, Read, Result},
    net::{SocketAddr, TcpStream},
    os::unix::prelude::ExitStatusExt,
    path::Path,
    process::{ExitStatus, Output},
    time::Duration,
};

pub struct Controller {
    session: Session,
}

impl Controller {
    pub fn new(
        addr: SocketAddr,
        timeout: Duration,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        let conn = TcpStream::connect_timeout(&addr, timeout)?;

        let mut session = Session::new()?;
        session.set_tcp_stream(conn);
        session.set_timeout(timeout.as_millis().try_into().unwrap_or(u32::MAX));
        session.handshake()?;
        session.userauth_password(username, password)?;

        Ok(Self { session })
    }

    pub fn exec(&mut self, cmd: &str) -> Result<Output> {
        let mut channel = self.session.channel_session()?;
        channel.exec(cmd)?;

        let mut stdout = Vec::new();
        channel.read_to_end(&mut stdout)?;

        let mut stderr = Vec::new();
        channel.stderr().read_to_end(&mut stderr)?;

        channel.wait_close()?;
        let exit_status = channel.exit_status()?;

        Ok(Output {
            status: ExitStatus::from_raw(exit_status),
            stdout,
            stderr,
        })
    }

    pub fn send<P1, P2>(&mut self, local: P1, remote: P2, mode: i32) -> Result<()>
    where
        P1: AsRef<Path>,
        P2: AsRef<Path>,
    {
        let mut file = File::open(local.as_ref())?;
        let size = file.metadata()?.len();

        let mut remote_file = self.session.scp_send(remote.as_ref(), mode, size, None)?;
        io::copy(&mut file, &mut remote_file)?;

        remote_file.send_eof()?;
        remote_file.wait_eof()?;
        remote_file.close()?;
        remote_file.wait_close()?;

        Ok(())
    }
}
