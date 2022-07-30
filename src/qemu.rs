use crate::{Output, Result, Timeout};
use std::{
    cmp,
    ffi::{OsStr, OsString},
    io::{self, BufRead, BufReader, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command},
    process::{ExitStatus, Stdio},
    str::FromStr,
    thread,
    time::Duration,
};
use tempfile::TempDir;

/// A struct for building new Qemu images.
pub struct ImageBuilder {
    /// Command invoked to create a new image.
    pub cmd: OsString,
}

impl ImageBuilder {
    /// Creates a new qcow2 image located at `dst` and backed by `src`.
    pub fn qcow2(&self, src: &OsStr, dst: &OsStr) -> Result<Output> {
        let mut image = OsString::new();
        image.push("backing_file=");
        image.push(src);

        Command::new(&self.cmd)
            .arg("create")
            .arg("-f")
            .arg("qcow2")
            .arg("-F")
            .arg("raw")
            .arg("-o")
            .arg(image)
            .arg(dst)
            .output()?
            .try_into()
    }
}

/// A struct for interacting with QEMU Monitor.
struct MonitorHandle {
    /// A temporary directory containing the UNIX socket used by the Monitor.
    socket_dir: TempDir,
}

impl MonitorHandle {
    /// Name of the UNIX socket file, fixed.
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
    fn ssh_port(&self) -> io::Result<u16> {
        let mut stream = UnixStream::connect(self.socket())?;

        stream.write_all(b"info usernet\n")?;
        stream.flush()?;
        stream.shutdown(Shutdown::Write)?;

        let mut buffered = BufReader::new(stream);
        let mut line = String::with_capacity(1024);

        loop {
            line.clear();
            if buffered.read_line(&mut line)? == 0 {
                break Err(io::Error::new(
                    io::ErrorKind::Other,
                    "no SSH port forward in network info received from the QEMU monitor",
                ));
            }

            let mut chunks = line.split_ascii_whitespace();
            let hostfwd = chunks
                .next()
                .map(|p| p.contains("HOST_FORWARD"))
                .unwrap_or(false);
            if hostfwd {
                let src_port = chunks.nth(2).map(u16::from_str).transpose().ok().flatten();
                let dst_port = chunks.nth(1).map(u16::from_str).transpose().ok().flatten();

                if let (Some(src), Some(22)) = (src_port, dst_port) {
                    break Ok(src);
                }
            }
        }
    }
}

/// A wrapper over a Qemu instance running as a [Child] process.
/// The instance is killed on drop.
pub struct QemuInstance {
    child: Option<Child>,
    image_path: OsString,
    monitor: MonitorHandle,
}

impl QemuInstance {
    /// Returns a [SocketAddr] for the SSH connection.
    pub fn ssh(&self) -> io::Result<SocketAddr> {
        let port = self.monitor.ssh_port()?;

        Ok(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port))
    }

    /// Returns a path to the QEMU image of this instance.
    pub fn image_path(&self) -> &OsStr {
        &self.image_path
    }

    /// Kills the wrapped [Child].
    pub fn kill(&mut self) -> io::Result<()> {
        self.child.as_mut().unwrap().kill()
    }

    /// Waits for the wrapped [Child]'s [Output].
    pub fn wait(mut self) -> Result<Output> {
        self.child.take().unwrap().wait_with_output()?.try_into()
    }

    /// Checks whether the wrapped [Child] has exited.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.as_mut().unwrap().try_wait()
    }
}

impl Drop for QemuInstance {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.kill().ok();
        }
    }
}

/// A struct used to spawn new [QemuInstance]s.
pub struct QemuSpawner {
    /// The command used to invoke QEMU.
    pub cmd: OsString,
    /// The memory limit for new instances (megabytes).
    pub memory: u16,
    /// Whether to enable KVM for new instances.
    pub enable_kvm: bool,
    /// Whether to turn of the kernel irqchip.
    pub irqchip_off: bool,
}

impl QemuSpawner {
    /// Prepares a [Command] to spawn a new instance.
    fn setup_cmd(&self, image_path: &OsStr, monitor_socket: &OsStr) -> Command {
        let mut drive = OsString::new();
        drive.push("file=");
        drive.push(image_path);

        let mut monitor = OsString::new();
        monitor.push("unix:");
        monitor.push(monitor_socket);
        monitor.push(",server,nowait");

        let mut cmd = Command::new(&self.cmd);
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
            .arg(format!("{}M", self.memory))
            .arg("-monitor")
            .arg(monitor);

        if self.enable_kvm {
            cmd.arg("-enable-kvm");
        }

        if self.irqchip_off {
            cmd.arg("-machine").arg("kernel_irqchip=off");
        }

        cmd.stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .stdin(Stdio::null());

        cmd
    }

    /// Spawns a new QEMU instance.
    /// The instance will use the image under the given `image_path`.
    /// This method will wait until the newly spawned QEMU creates a Monitor socket (but no longer than the given `monitor_timeout`).
    pub fn spawn(
        &self,
        image_path: OsString,
        monitor_timeout: Option<Duration>,
    ) -> Result<QemuInstance> {
        let monitor = MonitorHandle::new()?;
        let socket = monitor.socket();
        let child = self.setup_cmd(&image_path, socket.as_os_str()).spawn()?;

        let instance = QemuInstance {
            child: Some(child),
            image_path,
            monitor,
        };

        if let Some(timeout) = monitor_timeout {
            let timeout = Timeout::new(timeout);
            while !socket.exists() {
                thread::sleep(cmp::min(timeout.remaining()?, Duration::from_millis(50)));
            }
        }

        Ok(instance)
    }
}
