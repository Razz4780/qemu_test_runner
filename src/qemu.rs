use crate::{CanFail, Result};
use std::{
    ffi::{OsStr, OsString},
    io::{self, Error, ErrorKind},
    net::{Ipv4Addr, SocketAddr},
    process::{ExitStatus, Output, Stdio},
    sync::Arc,
};
use tokio::{
    process::{Child, Command},
    sync::{OwnedSemaphorePermit, Semaphore},
    task,
};

/// A struct for building new Qemu images.
pub struct ImageBuilder {
    /// Command invoked to create a new image.
    cmd: OsString,
}

impl ImageBuilder {
    /// Creates a new instance of this struct.
    /// This instance will use the given `cmd` to build images.
    pub fn new(cmd: OsString) -> Self {
        Self { cmd }
    }

    /// Creates a new qcow2 image located at `dst` and backed by `src`.
    pub async fn qcow2(&self, src: &OsStr, dst: &OsStr) -> Result<Output> {
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
            .output()
            .await?
            .result()
    }
}

/// A wrapper over a Qemu instance running as a [Child] process.
/// The instance is killed on drop.
pub struct QemuInstance {
    child: Option<Child>,
    permit: Option<OwnedSemaphorePermit>,
    image_path: OsString,
    ssh_port: u16,
}

impl QemuInstance {
    /// Returns a [SocketAddr] for the SSH connection.
    pub fn ssh(&self) -> SocketAddr {
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), self.ssh_port)
    }

    /// Returns a path to the Qemu image of this instance.
    pub fn image_path(&self) -> &OsStr {
        &self.image_path
    }

    /// Kills the wrapped [Child].
    pub async fn kill(&mut self) -> io::Result<()> {
        self.child.as_mut().unwrap().kill().await
    }

    /// Waits for the wrapped [Child]'s [Output].
    pub async fn wait(mut self) -> Result<Output> {
        self.child
            .take()
            .unwrap()
            .wait_with_output()
            .await?
            .result()
    }

    /// Checks whether the wrapped [Child] has exited.
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

/// A config for spawning new Qemu instances.
pub struct QemuConfig {
    /// The command used to invoke Qemu.
    pub cmd: OsString,
    /// The memory limit for new instances (megabytes).
    pub memory: u16,
    /// Whether to enable KVM for new instances.
    pub enable_kvm: bool,
    /// Whether to turn of the kernel irqchip.
    pub irqchip_off: bool,
}

/// A struct used to spawn new Qemu instances.
pub struct QemuSpawner {
    permits: Arc<Semaphore>,
    config: QemuConfig,
}

impl QemuSpawner {
    /// Creates a new instance of this struct.
    /// At any time there will be at most `children_limit` running Qemu processes spawned with this instance.
    pub fn new(children_limit: usize, config: QemuConfig) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(children_limit)),
            config,
        }
    }

    /// Prepares a [Command] to spawn a new instance.
    fn setup_cmd(&self, image_path: &OsStr, ssh_port: u16) -> Command {
        let mut drive = OsString::new();
        drive.push("file=");
        drive.push(image_path);

        let mut cmd = Command::new(&self.config.cmd);
        cmd.arg("-nographic")
            .arg("-drive")
            .arg(drive)
            .arg("-rtc")
            .arg("base=localtime")
            .arg("-net")
            .arg("nic,model=virtio")
            .arg("-net")
            .arg(format!("user,hostfwd=tcp::{}-:22", ssh_port))
            .arg("-m")
            .arg(format!("{}M", self.config.memory));

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

    /// Spawns a new Qemu instance.
    /// The instance will use the image under the given `image_path`.
    /// This method will wait if there are too many running Qemu processes spawned with this instance.
    pub async fn spawn(&self, image_path: OsString) -> Result<QemuInstance> {
        let ssh_port = portpicker::pick_unused_port()
            .ok_or_else(|| Error::new(ErrorKind::Other, "no free port"))?;
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore should not be closed");
        let child = self.setup_cmd(&image_path, ssh_port).spawn()?;

        Ok(QemuInstance {
            child: Some(child),
            permit: Some(permit),
            image_path,
            ssh_port,
        })
    }
}
