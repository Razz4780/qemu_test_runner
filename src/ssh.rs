use crate::{Error, Output, Result, Timeout};
use ssh2::Session;
use std::{
    fs::File,
    io::{self, Read},
    net::{SocketAddr, TcpStream},
    path::Path,
    thread,
    time::Duration,
};

/// A handle to the open SSH [Session].
pub struct SshHandle {
    /// The active SSH session.
    session: Session,
}

impl SshHandle {
    /// Opens a new [Session] with the given parameters.
    fn open_session(
        addr: SocketAddr,
        username: &str,
        password: &str,
        timeout: &Timeout,
    ) -> io::Result<Session> {
        let conn = TcpStream::connect_timeout(&addr, timeout.remaining()?)?;

        let mut session = Session::new()?;
        session.set_tcp_stream(conn);
        session.set_timeout(timeout.remaining_ms()?);
        session.handshake()?;
        session.set_timeout(timeout.remaining_ms()?);
        session.userauth_password(username, password)?;

        Ok(session)
    }

    /// Opens a new SSH [Session] and creates a new instance of this struct.
    pub fn new(
        addr: SocketAddr,
        username: String,
        password: String,
        timeout: Duration,
    ) -> io::Result<Self> {
        let timeout = Timeout::new(timeout);
        let session = loop {
            let res = Self::open_session(addr, &username, &password, &timeout);
            if let Ok(res) = res {
                break res;
            }
            thread::sleep(Duration::from_millis(100));
        };

        Ok(Self { session })
    }

    /// Executes a command on the remote machine.
    pub fn exec(&mut self, cmd: &str, timeout: Duration) -> Result<Output> {
        let timeout = Timeout::new(timeout);

        self.session.set_timeout(timeout.remaining_ms()?);
        let mut channel = self.session.channel_session()?;
        self.session.set_timeout(timeout.remaining_ms()?);
        channel.exec(cmd).map_err(io::Error::from)?;

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

        if exit_status == 0 {
            Ok(Output { stdout, stderr })
        } else {
            Err(Error {
                error: io::Error::from_raw_os_error(exit_status).into(),
                stdout,
                stderr,
            })
        }
    }

    /// Transfers a file to the remote machine.
    pub fn send(
        &mut self,
        local: &Path,
        remote: &Path,
        mode: i32,
        timeout: Duration,
    ) -> Result<()> {
        let timeout = Timeout::new(timeout);

        let mut file = File::open(local)?;
        let size = file.metadata()?.len();

        self.session.set_timeout(timeout.remaining_ms()?);
        let mut remote_file = self.session.scp_send(remote, mode, size, None)?;
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
