use crate::{Error, Output, Result, Timeout};
use ssh2::Session;
use std::{
    fs::File,
    io::{self, Read},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task,
};

/// A command that can be executed by the [SshHandle].
#[derive(Debug)]
enum SshCommand {
    /// Executing a command on the remote machine.
    Exec {
        /// Commang to be executed.
        cmd: String,
        /// Command timeout.
        timeout: Duration,
        /// Result channel.
        tx: oneshot::Sender<Result<Output>>,
    },
    /// Sending a file to the remote machine.
    Send {
        /// Local path to the source file.
        local: PathBuf,
        /// Remote path to the destination.
        remote: PathBuf,
        /// UNIX permissions of the destination file.
        mode: i32,
        /// File transfer timeout.
        timeout: Duration,
        /// Result channel.
        tx: oneshot::Sender<io::Result<()>>,
    },
}

/// A worker for executing blocking functions from the [ssh2] crate.
struct SshWorker {
    /// The active SSH session.
    session: Session,
    /// The channel for new [SshCommand]s.
    receiver: mpsc::Receiver<SshCommand>,
}

impl SshWorker {
    /// Opens a new [Session] with the given parameters.
    /// This is a blocking method.
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

    /// Creates a new instance of this struct.
    async fn new(
        addr: SocketAddr,
        username: String,
        password: String,
        timeout: Duration,
        receiver: mpsc::Receiver<SshCommand>,
    ) -> io::Result<Self> {
        let open = move || -> io::Result<Session> {
            let timeout = Timeout::new(timeout);
            loop {
                let res = Self::open_session(addr, &username, &password, &timeout);
                if res.is_ok() {
                    break res;
                }
                thread::sleep(Duration::from_secs(1));
            }
        };
        let session = task::spawn_blocking(open).await.unwrap()?;

        Ok(Self { session, receiver })
    }

    /// Runs this worker until all of the related [SshCommand] [mpsc::Sender]s are dropped.
    /// This is a blocking method.
    fn run(mut self) {
        while let Some(command) = self.receiver.blocking_recv() {
            match command {
                SshCommand::Exec { cmd, timeout, tx } => {
                    let res = self.exec(&cmd, timeout);
                    tx.send(res).ok();
                }
                SshCommand::Send {
                    local,
                    remote,
                    mode,
                    timeout,
                    tx,
                } => {
                    let res = self.send(&local, &remote, mode, timeout);
                    tx.send(res).ok();
                }
            }
        }
    }

    /// Executed a command on the remote machine.
    /// This is a blocking method.
    fn exec(&mut self, cmd: &str, timeout: Duration) -> Result<Output> {
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
    /// This is a blocking method.
    fn send(
        &mut self,
        local: &Path,
        remote: &Path,
        mode: i32,
        timeout: Duration,
    ) -> io::Result<()> {
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

/// A handle to the running [SshWorker].
pub struct SshHandle {
    /// The channel for sending [SshCommand]s to the worker.
    sender: mpsc::Sender<SshCommand>,
}

impl SshHandle {
    /// Spawns a new [SshWorker] in the background and returns a new handle for it.
    pub async fn new(
        addr: SocketAddr,
        username: String,
        password: String,
        timeout: Duration,
    ) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel(1);

        let worker = SshWorker::new(addr, username, password, timeout, rx).await?;
        task::spawn_blocking(move || worker.run());

        Ok(Self { sender: tx })
    }

    /// Executes a command on the remote machine.
    pub async fn exec(&mut self, cmd: String, timeout: Duration) -> Result<Output> {
        let (tx, rx) = oneshot::channel();

        self.sender
            .send(SshCommand::Exec { cmd, timeout, tx })
            .await
            .expect("ssh worker died");

        rx.await.expect("ssh worker died")
    }

    /// Transfers a file to the remote machine.
    pub async fn send(
        &mut self,
        local: PathBuf,
        remote: PathBuf,
        mode: i32,
        timeout: Duration,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();

        self.sender
            .send(SshCommand::Send {
                local,
                remote,
                mode,
                timeout,
                tx,
            })
            .await
            .expect("ssh worker died");

        rx.await.expect("ssh worker died").map_err(Into::into)
    }
}
