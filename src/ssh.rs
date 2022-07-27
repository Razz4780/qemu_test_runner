use crate::{CanFail, Result, Timeout};
use ssh2::Session;
use std::{
    fs::File,
    io::{self, Read},
    net::{SocketAddr, TcpStream},
    os::unix::prelude::ExitStatusExt,
    path::{Path, PathBuf},
    process::{ExitStatus, Output},
    thread,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task,
};

#[derive(Debug)]
enum SshCommand {
    Exec {
        cmd: String,
        timeout: Duration,
        tx: oneshot::Sender<Result<Output>>,
    },
    Send {
        local: PathBuf,
        remote: PathBuf,
        mode: i32,
        timeout: Duration,
        tx: oneshot::Sender<io::Result<()>>,
    },
}

struct SshWorker {
    session: Session,
    receiver: mpsc::Receiver<SshCommand>,
}

impl SshWorker {
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

        Ok(Output {
            status: ExitStatus::from_raw(exit_status),
            stdout,
            stderr,
        })
    }

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

pub struct SshHandle {
    sender: mpsc::Sender<SshCommand>,
}

impl SshHandle {
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

    pub async fn exec(&mut self, cmd: String, timeout: Duration) -> Result<Output> {
        let (tx, rx) = oneshot::channel();

        self.sender
            .send(SshCommand::Exec { cmd, timeout, tx })
            .await
            .expect("ssh worker died");

        rx.await.expect("ssh worker died")?.result()
    }

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
