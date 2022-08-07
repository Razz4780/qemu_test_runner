use crate::{Error, Output};
use serde::Deserialize;
use serde::Serialize;
use ssh2::Session;
use std::{
    fmt::Display,
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
#[derive(Debug, Deserialize, Clone, Serialize)]
pub enum SshAction {
    /// Executing a command on the remote machine.
    Exec {
        /// Commang to be executed.
        cmd: String,
    },
    /// Sending a file to the remote machine.
    Send {
        /// Path to the source on the local machine.
        from: PathBuf,
        /// Path to the destination on the remote machine.
        to: PathBuf,
        /// UNIX permissions of the destination file.
        mode: i32,
    },
}

struct Work(SshAction, oneshot::Sender<Result<Output, Error>>);

/// A worker for executing blocking functions from the [ssh2] crate.
struct SshWorker {
    /// The active SSH session.
    session: Session,
    /// The channel for new [Work] to do.
    receiver: mpsc::Receiver<Work>,
    /// Limit for stdout and stderr of executed commands.
    output_limit: Option<u64>,
}

impl SshWorker {
    /// Opens a new [Session] with the given parameters.
    /// This is a blocking method.
    fn open_session(addr: SocketAddr, username: &str, password: &str) -> io::Result<Session> {
        let conn = TcpStream::connect(&addr)?;

        let mut session = Session::new()?;
        session.set_tcp_stream(conn);
        session.handshake()?;
        session.userauth_password(username, password)?;

        Ok(session)
    }

    /// Runs this worker until all of the related [SshAction] [mpsc::Sender]s are dropped.
    /// This is a blocking method.
    fn run(mut self) {
        while let Some(Work(action, tx)) = self.receiver.blocking_recv() {
            match action {
                SshAction::Exec { cmd } => {
                    let res = self.exec(&cmd);
                    tx.send(res).ok();
                }
                SshAction::Send { from, to, mode } => {
                    let res = self
                        .send(&from, &to, mode)
                        .map(|_| Output::default())
                        .map_err(Into::into);
                    tx.send(res).ok();
                }
            }
        }
    }

    /// Executes a command on the remote machine.
    /// This is a blocking method.
    fn exec(&mut self, cmd: &str) -> Result<Output, Error> {
        let mut channel = self.session.channel_session()?;
        channel.exec(cmd).map_err(io::Error::from)?;

        let mut stdout = Vec::new();
        match self.output_limit {
            Some(limit) => (&mut channel).take(limit).read_to_end(&mut stdout)?,
            None => channel.read_to_end(&mut stdout)?,
        };

        let mut stderr = Vec::new();
        match self.output_limit {
            Some(limit) => channel.stderr().take(limit).read_to_end(&mut stderr)?,
            None => channel.stderr().read_to_end(&mut stderr)?,
        };

        channel.wait_close()?;
        let exit_status = channel.exit_status()?;

        if exit_status == 0 {
            Ok(Output {
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            })
        } else {
            Err(Error {
                error: io::Error::from_raw_os_error(exit_status).into(),
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            })
        }
    }

    /// Transfers a file to the remote machine.
    /// This is a blocking method.
    fn send(&mut self, local: &Path, remote: &Path, mode: i32) -> io::Result<()> {
        let mut file = File::open(local)?;
        let size = file.metadata()?.len();

        let mut remote_file = self.session.scp_send(remote, mode, size, None)?;
        io::copy(&mut file, &mut remote_file)?;

        remote_file.send_eof()?;
        remote_file.wait_eof()?;
        remote_file.close()?;
        remote_file.wait_close()?;

        Ok(())
    }
}

pub struct SshHandle {
    /// The channel for sending [Work] to the worker.
    sender: mpsc::Sender<Work>,
}

impl SshHandle {
    /// Creates a new instance of this struct.
    pub async fn new(
        addr: SocketAddr,
        username: String,
        password: String,
        output_limit: Option<u64>,
    ) -> io::Result<Self> {
        let session = task::spawn_blocking(move || loop {
            let res = SshWorker::open_session(addr, &username, &password);
            if let Ok(session) = res {
                break session;
            }

            thread::sleep(Duration::from_millis(100));
        })
        .await
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("failed to open an SSH connection: {}", e),
            )
        })?;

        let (tx, rx) = mpsc::channel(1);

        let worker = SshWorker {
            session,
            receiver: rx,
            output_limit,
        };
        task::spawn_blocking(move || worker.run());

        Ok(Self { sender: tx })
    }

    fn worker_died<E>(error: E) -> io::Error
    where
        E: Display,
    {
        io::Error::new(
            io::ErrorKind::Other,
            format!("SSH worker unexpectedly died: {}", error),
        )
    }

    /// Executes an [SshAction] on the remote machine.
    pub async fn exec(&mut self, cmd: SshAction) -> Result<Output, Error> {
        let (tx, rx) = oneshot::channel();

        self.sender
            .send(Work(cmd, tx))
            .await
            .map_err(Self::worker_died)?;

        rx.await.map_err(Self::worker_died)?
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{qemu::Image, test_util::Env};
    use tokio::{fs, time};

    #[ignore]
    #[tokio::test]
    async fn ls_and_poweroff() {
        time::timeout(Duration::from_secs(60), async {
            let env = Env::read();

            let image = env.base_path().join("image.qcow2");

            env.builder()
                .create(env.base_image(), Image::Qcow2(image.as_path()))
                .await
                .expect("failed to build the image");
            let qemu = env
                .spawner(1)
                .spawn(image.into())
                .await
                .expect("failed to spawn the QEMU process");

            let ssh_addr = qemu.ssh().await.expect("failed to get the ssh address");

            let mut ssh_handle = SshHandle::new(ssh_addr, "root".into(), "root".into(), None)
                .await
                .expect("failed to get the ssh handle");

            ssh_handle
                .exec(SshAction::Exec { cmd: "ls".into() })
                .await
                .expect("ls failed");
            ssh_handle
                .exec(SshAction::Exec {
                    cmd: "/sbin/poweroff".into(),
                })
                .await
                .ok();

            qemu.wait().await.expect("QEMU process failed");
        })
        .await
        .expect("timeout");
    }

    #[ignore]
    #[tokio::test]
    async fn file_transfer() {
        time::timeout(Duration::from_secs(60), async {
            let env = Env::read();

            let image = env.base_path().join("image.qcow2");

            env.builder()
                .create(env.base_image(), Image::Qcow2(image.as_path()))
                .await
                .expect("failed to build the image");
            let qemu = env
                .spawner(1)
                .spawn(image.into())
                .await
                .expect("failed to spawn the QEMU process");

            let ssh_addr = qemu.ssh().await.expect("failed to get the ssh address");

            let mut ssh_handle = SshHandle::new(ssh_addr, "root".into(), "root".into(), None)
                .await
                .expect("failed to get the ssh handle");

            let file_path = env.base_path().join("file");
            fs::write(&file_path, b"content")
                .await
                .expect("writing to file failed");
            ssh_handle
                .exec(SshAction::Send {
                    from: file_path.clone(),
                    to: "dst".into(),
                    mode: 0o777,
                })
                .await
                .expect("file transfer failed");
            let output = ssh_handle
                .exec(SshAction::Exec {
                    cmd: "cat dst".into(),
                })
                .await
                .expect("ls failed");
            assert!(output.stdout.contains("content"),);
            ssh_handle
                .exec(SshAction::Exec {
                    cmd: "/sbin/poweroff".into(),
                })
                .await
                .ok();

            qemu.wait().await.expect("QEMU process failed");
        })
        .await
        .expect("timeout");
    }
}
