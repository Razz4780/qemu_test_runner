use crate::Output;
use serde::{Deserialize, Serialize};
use ssh2::Session;
use std::{
    fmt::Display,
    fs::File,
    io::{self, Read},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task,
};

/// A command that can be executed by the [SshHandle].
#[derive(Debug, Deserialize, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
    },
}

struct Work(SshAction, oneshot::Sender<Output>);

/// A worker for executing blocking functions from the [ssh2] crate.
struct SshWorker {
    /// The active SSH session.
    session: Session,
    /// The channel for new [Work] to do.
    receiver: mpsc::Receiver<Work>,
    /// Limit for stdout and stderr of executed commands.
    /// The output will be truncated to this length.
    output_limit: Option<u64>,
}

impl SshWorker {
    /// Opens a new [Session] with the given parameters.
    /// This is a blocking method.
    /// # Arguments
    /// addr - [SocketAddr] to connect to.
    /// username - username of the user to authenticate.
    /// password - password of the user to authenticate.
    /// # Returns
    /// A new SSH [Session].
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
            let res = match action {
                SshAction::Exec { cmd } => self.exec(&cmd),
                SshAction::Send { from, to } => self.send(&from, &to).map(|_| Output::Finished {
                    exit_code: 0,
                    stdout: Default::default(),
                    stderr: Default::default(),
                }),
            };

            let output = match res {
                Ok(output) => output,
                Err(error) => Output::Error { error },
            };

            tx.send(output).ok();
        }
    }

    /// Executes a command on the remote machine.
    /// This is a blocking method.
    /// # Arguments
    /// cmd - the command to execute.
    /// # Returns
    /// The [Output] of the command.
    fn exec(&mut self, cmd: &str) -> io::Result<Output> {
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
        let exit_code = channel.exit_status()?;

        Ok(Output::Finished {
            exit_code,
            stdout,
            stderr,
        })
    }

    /// Transfers a file to the remote machine.
    /// This is a blocking method.
    /// # Arguments
    /// local - path to the source file on the local machine.
    /// remote - path to the destination file on the remote machine.
    fn send(&mut self, local: &Path, remote: &Path) -> io::Result<()> {
        let mut file = File::open(local)?;
        let size = file.metadata()?.len();

        let mut remote_file = self.session.scp_send(remote, 0o777, size, None)?;
        io::copy(&mut file, &mut remote_file)?;

        remote_file.send_eof()?;
        remote_file.wait_eof()?;
        remote_file.close()?;
        remote_file.wait_close()?;

        Ok(())
    }
}

/// A handle for executing [SshAction]s on a remote machine.
pub struct SshHandle {
    /// The channel for sending [Work] to the worker.
    sender: mpsc::Sender<Work>,
}

impl SshHandle {
    /// # Arguments
    /// addr - [SocketAddr] of the SSH server.
    /// username - username of the user to authenticate.
    /// password - password of the user to authenticate.
    /// output_limit - limit for stdin and stderr of executed commands.
    /// # Returns
    /// A new instance of this struct.
    pub async fn new(
        addr: SocketAddr,
        username: String,
        password: String,
        output_limit: Option<u64>,
    ) -> io::Result<Self> {
        let session = {
            log::debug!("Establishing an SSH connection to {}.", addr);
            let guard = Arc::new(());
            let weak = Arc::downgrade(&guard);
            task::spawn_blocking(move || {
                while weak.strong_count() > 0 {
                    if let Ok(session) = SshWorker::open_session(addr, &username, &password) {
                        return Some(session);
                    }
                    thread::sleep(Duration::from_millis(100));
                }

                None
            })
            .await
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("failed to open an SSH connection: {}", e),
                )
            })?
            .expect("task was not cancelled")
        };

        let (tx, rx) = mpsc::channel(1);

        let worker = SshWorker {
            session,
            receiver: rx,
            output_limit,
        };
        log::debug!("Spawning a background SSH worker for address {}.", addr);
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
    /// # Arguments
    /// cmd - action to execute.
    /// # Returns
    /// [Output] of the executed action.
    pub async fn exec(&mut self, cmd: SshAction) -> io::Result<Output> {
        let (tx, rx) = oneshot::channel();

        self.sender
            .send(Work(cmd, tx))
            .await
            .map_err(Self::worker_died)?;

        rx.await.map_err(Self::worker_died)
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
            let output = ssh_handle
                .exec(SshAction::Send {
                    from: file_path.clone(),
                    to: "dst".into(),
                })
                .await
                .unwrap();
            assert!(output.success());
            let output = ssh_handle
                .exec(SshAction::Exec {
                    cmd: "cat dst".into(),
                })
                .await
                .unwrap();
            assert!(output.success());
            let stdout = output.stdout().expect("stdout should exist");
            assert!(String::from_utf8_lossy(stdout).contains("content"),);
            let output = ssh_handle
                .exec(SshAction::Exec {
                    cmd: "/sbin/poweroff".into(),
                })
                .await
                .unwrap();
            assert!(!output.success());

            qemu.wait().await.expect("QEMU process failed");
        })
        .await
        .expect("timeout");
    }
}
