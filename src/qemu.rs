use std::{
    ffi::{OsStr, OsString},
    io::{Error, ErrorKind, Result},
    net::{Ipv4Addr, SocketAddr},
    os::unix::process::ExitStatusExt,
    process::{ExitStatus, Output, Stdio},
    sync::Arc,
};
use tokio::{
    process::{Child, Command},
    sync::{OwnedSemaphorePermit, Semaphore},
    task,
};

pub struct ImageBuilder {
    /// Command invoked to create a new image.
    cmd: OsString,
}

impl ImageBuilder {
    pub fn new(cmd: OsString) -> Self {
        Self { cmd }
    }

    /// Creates a new qcow2 image located at `dst` and backed by `src`.
    pub async fn qcow2(&self, src: &OsStr, dst: &OsStr) -> Result<()> {
        let mut image = OsString::new();
        image.push("backing_file=");
        image.push(src);

        let output = Command::new(&self.cmd)
            .arg("create")
            .arg("-f")
            .arg("qcow2")
            .arg("-F")
            .arg("raw")
            .arg("-o")
            .arg(image)
            .arg(dst)
            .output()
            .await?;

        if output.status.success() {
            Ok(())
        } else {
            Err(Error::from_raw_os_error(output.status.into_raw()))
        }
    }
}

pub struct QemuInstance {
    child: Option<Child>,
    permit: Option<OwnedSemaphorePermit>,
    image_path: OsString,
    ssh_port: u16,
}

impl QemuInstance {
    pub fn ssh(&self) -> SocketAddr {
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), self.ssh_port)
    }

    pub fn image_path(&self) -> &OsStr {
        &self.image_path
    }

    pub async fn kill(&mut self) -> Result<()> {
        self.child.as_mut().unwrap().kill().await
    }

    pub async fn wait(mut self) -> Result<Output> {
        self.child.take().unwrap().wait_with_output().await
    }

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
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

pub struct QemuConfig {
    pub cmd: OsString,
    pub memory: u16,
    pub enable_kvm: bool,
    pub irqchip_off: bool,
}

pub struct QemuSpawner {
    permits: Arc<Semaphore>,
    config: QemuConfig,
}

impl QemuSpawner {
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
