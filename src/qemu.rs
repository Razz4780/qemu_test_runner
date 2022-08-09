use std::{
    ffi::{OsStr, OsString},
    io,
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    path::PathBuf,
    process::{ExitStatus, Stdio},
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    process::{Child, Command},
    sync::{OwnedSemaphorePermit, Semaphore},
    task, time,
};

/// An image for QEMU process.
#[derive(Clone, Copy)]
pub enum Image<'a> {
    /// A copy-on-write image.
    Qcow2(&'a Path),
    /// A raw image.
    Raw(&'a Path),
}

impl<'a> Image<'a> {
    /// # Returns
    /// The path to the image file.
    pub fn path(self) -> &'a Path {
        match self {
            Self::Qcow2(p) => p,
            Self::Raw(p) => p,
        }
    }

    fn format(self) -> &'static OsStr {
        match self {
            Self::Qcow2(_) => "qcow2".as_ref(),
            Self::Raw(_) => "raw".as_ref(),
        }
    }
}

/// A struct for building new QEMU images.
pub struct ImageBuilder {
    /// Command invoked to create a new image.
    pub cmd: OsString,
}

impl ImageBuilder {
    /// Creates a new copy-on-write image.
    /// # Arguments
    /// src - source (backing) image.
    /// dst - destination (backed) image.
    pub async fn create(&self, src: Image<'_>, dst: Image<'_>) -> io::Result<()> {
        Command::new(&self.cmd)
            .arg("create")
            .arg("-f")
            .arg(dst.format())
            .arg("-b")
            .arg(src.path())
            .arg("-F")
            .arg(src.format())
            .arg(dst.path())
            .output()
            .await?;

        Ok(())
    }
}

/// A struct for interacting with QEMU Monitor.
struct MonitorHandle {
    /// A temporary directory containing the UNIX socket used by the Monitor.
    socket_dir: TempDir,
}

impl MonitorHandle {
    /// Name of the UNIX socket file.
    const SOCKET_NAME: &'static str = "monitor.sock";

    /// Creates a new instance of this struct.
    /// Creates a temporary directory for the socket file, but does not create the socket itself.
    /// It must be created by the QEMU.
    fn new() -> io::Result<Self> {
        let socket_dir = tempfile::tempdir()?;

        Ok(Self { socket_dir })
    }

    /// Returns the path to the UNIX socket.
    /// This path may not exist yet, the socket should be created by the QEMU.
    fn socket(&self) -> PathBuf {
        self.socket_dir.path().join(Self::SOCKET_NAME)
    }

    /// Returns the number of the local port forwarded to the port 22 (standard SSH port).
    async fn ssh_port(&self) -> io::Result<u16> {
        let mut stream = {
            let socket = self.socket();
            while !socket.exists() {
                time::sleep(Duration::from_millis(100)).await;
            }
            UnixStream::connect(socket).await?
        };

        stream.write_all(b"info usernet\n").await?;
        stream.flush().await?;
        stream.shutdown().await?;

        let mut buffered = BufReader::new(stream);
        let mut line = String::with_capacity(1024);

        while buffered.read_line(&mut line).await? > 0 {
            let mut chunks = line.split_ascii_whitespace();
            let hostfwd = chunks
                .next()
                .map(|p| p.contains("HOST_FORWARD"))
                .unwrap_or(false);
            if hostfwd {
                let src_port = chunks.nth(2).map(u16::from_str).transpose().ok().flatten();
                let dst_port = chunks.nth(1).map(u16::from_str).transpose().ok().flatten();

                if let (Some(src), Some(22)) = (src_port, dst_port) {
                    return Ok(src);
                }
            }

            line.clear();
        }

        Err(io::Error::new(
            io::ErrorKind::Other,
            "no SSH port forward found in network info received from the QEMU monitor",
        ))
    }
}

/// A wrapper over a Qemu instance running as a [Child] process.
/// The instance is killed on drop.
pub struct QemuInstance {
    child: Option<Child>,
    permit: Option<OwnedSemaphorePermit>,
    image_path: OsString,
    monitor: MonitorHandle,
}

impl QemuInstance {
    /// # Returns
    /// A [SocketAddr] for the SSH connection with the wrapped QEMU instance.
    pub async fn ssh(&self) -> io::Result<SocketAddr> {
        let port = self.monitor.ssh_port().await?;

        Ok(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port))
    }

    /// # Returns
    /// The path to the QEMU image used by the wrapped instance.
    pub fn image_path(&self) -> &OsStr {
        &self.image_path
    }

    /// Kills the wrapped [Child].
    pub async fn kill(&mut self) -> io::Result<()> {
        self.child.as_mut().unwrap().kill().await
    }

    /// Waits for the wrapped [Child] to exit.
    pub async fn wait(mut self) -> io::Result<()> {
        let output = self.child.take().unwrap().wait_with_output().await?;
        if output.status.success() {
            Ok(())
        } else if let Some(code) = output.status.code() {
            log::warn!(
                "QEMU process [{}] unexpectedly ended with error code {}.",
                self.image_path.to_string_lossy(),
                code
            );
            Err(io::Error::new(
                io::ErrorKind::Other,
                format!("process exited with a non-zero code {}", code),
            ))
        } else {
            log::warn!(
                "QEMU process [{}] was unexpectedly killed by a signal.",
                self.image_path.to_string_lossy()
            );
            Err(io::Error::new(
                io::ErrorKind::Other,
                "process killed by a signal",
            ))
        }
    }

    /// Checks whether the wrapped [Child] has exited.
    /// # Returns
    /// Exit status of the wrapped [Child], if available.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.as_mut().unwrap().try_wait()
    }
}

impl Drop for QemuInstance {
    fn drop(&mut self) {
        let permit = self.permit.take();
        if let Some(mut child) = self.child.take() {
            child.start_kill().ok();
            task::spawn(async move {
                let _permit = permit;
                child.wait().await.ok();
            });
        }
    }
}

/// A config for spawning new [QemuInstance]s.
pub struct QemuConfig {
    /// The command used to spawn a QEMU process.
    pub cmd: OsString,
    /// The memory limit for new instances (megabytes).
    pub memory: u16,
    /// Whether to enable KVM for new instances.
    pub enable_kvm: bool,
    /// Whether to turn of the kernel irqchip.
    pub irqchip_off: bool,
}

/// A struct used to spawn new [QemuInstance]s.
pub struct QemuSpawner {
    permits: Arc<Semaphore>,
    config: QemuConfig,
}

impl QemuSpawner {
    /// # Arguments
    /// * children_limit - limit for concurrently running QEMU processes.
    /// * config - configuration for spawning new QEMU processes.
    /// # Returns
    /// A new instance of this struct.
    /// At any time there will be at most `children_limit` running QEMU processes
    /// spawned with this instance.
    pub fn new(children_limit: usize, config: QemuConfig) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(children_limit)),
            config,
        }
    }

    fn setup_cmd(&self, image_path: &OsStr, monitor_socket: &OsStr) -> Command {
        let mut drive = OsString::new();
        drive.push("file=");
        drive.push(image_path);

        let mut monitor = OsString::new();
        monitor.push("unix:");
        monitor.push(monitor_socket);
        monitor.push(",server,nowait");

        let mut cmd = Command::new(&self.config.cmd);
        cmd.arg("-nographic")
            .arg("-drive")
            .arg(drive)
            .arg("-rtc")
            .arg("base=localtime")
            .arg("-net")
            .arg("nic,model=virtio")
            .arg("-net")
            .arg("user,hostfwd=tcp::0-:22")
            .arg("-m")
            .arg(format!("{}M", self.config.memory))
            .arg("-monitor")
            .arg(monitor);

        if self.config.enable_kvm {
            cmd.arg("-enable-kvm");
        }

        if self.config.irqchip_off {
            cmd.arg("-machine").arg("kernel_irqchip=off");
        }

        cmd.stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .stdin(Stdio::null());

        cmd
    }

    /// Spawns a new QEMU instance.
    /// This method will wait if there are too many running QEMU processes spawned with this instance.
    /// # Arguments
    /// * image_path - path to the QEMU image to use.
    /// # Returns
    /// A newly spawned QEMU processed wrapped in a [QemuInstance].
    pub async fn spawn(&self, image_path: OsString) -> io::Result<QemuInstance> {
        log::debug!(
            "Awaiting for a permission to spawn a QEMU process on image {}.",
            image_path.to_string_lossy()
        );
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore should not be closed");

        let monitor = MonitorHandle::new()?;
        let socket = monitor.socket();

        log::debug!(
            "Spawning a QEMU process on image {}.",
            image_path.to_string_lossy()
        );
        let child = self.setup_cmd(&image_path, socket.as_os_str()).spawn()?;

        Ok(QemuInstance {
            child: Some(child),
            permit: Some(permit),
            image_path,
            monitor,
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_util::Env;
    use tokio::{task, time};

    #[ignore]
    #[tokio::test]
    async fn build_and_run() {
        let env = Env::read();

        let image = env.base_path().join("image.qcow2");

        env.builder()
            .create(env.base_image(), Image::Qcow2(image.as_path()))
            .await
            .expect("failed to build the image");
        let mut qemu = env
            .spawner(1)
            .spawn(image.into())
            .await
            .expect("failed to spawn the QEMU process");

        time::sleep(Duration::from_secs(1)).await;
        assert!(qemu.try_wait().expect("try_wait failed").is_none());
        qemu.kill().await.expect("kill failed");
        assert!(qemu.wait().await.is_err());
    }

    #[ignore]
    #[tokio::test]
    async fn spawner_concurrency_limit() {
        let env = Env::read();

        let image_1 = env.base_path().join("image_1.qcow2");
        let image_2 = env.base_path().join("image_2.qcow2");

        let builder = env.builder();
        for image in [image_1.as_path(), image_2.as_path()] {
            builder
                .create(env.base_image(), Image::Qcow2(image))
                .await
                .expect("failed to build the image");
        }

        let spawner = env.spawner(1);
        let _qemu = spawner
            .spawn(image_1.into())
            .await
            .expect("failed to spawn the QEMU process");

        let handle = task::spawn(async move { spawner.spawn(image_2.into()).await });

        time::sleep(Duration::from_secs(1)).await;
        assert!(!handle.is_finished());
    }
}
