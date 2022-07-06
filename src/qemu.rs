use std::{
    ffi::OsStr,
    fmt::{self, Display, Formatter},
    io::Result,
    process::{Child, Command, Output},
};

/// A command for building QEMU images.
pub struct Build<S> {
    /// Command invoked to create a new image.
    cmd: S,
}

impl Default for Build<&'static str> {
    /// Creates a new instance of this struct.
    /// The instance will use `qemu-img` to create images.
    fn default() -> Self {
        Self { cmd: "qemu-img" }
    }
}

impl<S> Build<S> {
    /// Sets the command invoked by this struct to create a new image.
    pub fn cmd(&mut self, cmd: S) -> &mut Self {
        self.cmd = cmd;
        self
    }
}

impl<S> Build<S>
where
    S: AsRef<OsStr>,
{
    /// Creates a new qcow2 image located at `dst` and backed by `src`.
    pub fn qcow2(&self, src: &str, dst: &str) -> Result<Output> {
        Command::new(self.cmd.as_ref())
            .arg("create")
            .arg("-f")
            .arg("qcow2")
            .arg("-F")
            .arg("raw")
            .arg("-o")
            .arg(format!("backing_file={}", src))
            .arg(dst)
            .output()
    }
}

/// An internet protocol for port forwarding.
pub enum Protocol {
    Tcp,
    Udp,
}

impl Display for Protocol {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp => f.write_str("tcp"),
            Self::Udp => f.write_str("udp"),
        }
    }
}

/// A direction for port forwarding.
pub enum Direction {
    /// Forwarding from a host port to a guest port.
    Hostfwd,
    /// Forwarding from a guest port to a host port.
    Guestfwd,
}

impl Display for Direction {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hostfwd => f.write_str("hostfwd"),
            Self::Guestfwd => f.write_str("guestfwd"),
        }
    }
}

/// A port forwarding rule.
pub struct Forward {
    /// Internet protocol.
    pub protocol: Protocol,
    /// Forward direction.
    pub direction: Direction,
    /// Port to forward from.
    pub from: u16,
    /// Port to forward to.
    pub to: u16,
}

impl Forward {
    /// Creates a new instance of this struct.
    /// The instance will represent a forwarding rule with:
    /// * protocol = [Protocol::Tcp],
    /// * direction = [Direction::Hostfwd],
    /// * from = a random unused port,
    /// * to = 22.
    /// This should enable connecting to the running instance with SSH.
    /// Returns [None] when no unused port can be found.
    pub fn new_ssh() -> Option<Self> {
        let port = portpicker::pick_unused_port()?;
        Some(Self {
            protocol: Protocol::Tcp,
            direction: Direction::Hostfwd,
            from: port,
            to: 22,
        })
    }
}

/// A command for running QEMU images.
pub struct Run<S> {
    /// Command invoked to run an image.
    cmd: S,
    /// Memory available to the image (megabytes).
    memory: u16,
    /// Whether to use kvm or not.
    enable_kvm: bool,
    /// Port forwarding rules.
    forwards: Vec<Forward>,
    /// Whether do turn off the kernel irqchip.
    irqchip_off: bool,
}

impl Default for Run<&'static str> {
    /// Creates a new instance of this struct.
    /// The instance will:
    /// * use `qemu-system-x86_64` command to run images,
    /// * set the available memory to 1024M,
    /// * enable kvm,
    /// * not forward any port,
    /// * turn off the kernel irqchip.
    fn default() -> Self {
        Self {
            cmd: "qemu-system-x86_64",
            memory: 1024,
            enable_kvm: true,
            forwards: Default::default(),
            irqchip_off: true,
        }
    }
}

impl<S> Run<S> {
    /// Sets the command invoked by this struct to run an image.
    pub fn cmd(&mut self, cmd: S) -> &mut Self {
        self.cmd = cmd;
        self
    }

    /// Sets the available memory.
    pub fn memory(&mut self, memory: u16) -> &mut Self {
        self.memory = memory;
        self
    }

    /// Enables or disables kvm.
    pub fn kvm(&mut self, enabled: bool) -> &mut Self {
        self.enable_kvm = enabled;
        self
    }

    /// Adds a port forward rule.
    pub fn forward(&mut self, forward: Forward) -> &mut Self {
        self.forwards.push(forward);
        self
    }

    /// Turns the kernel irqchip off or on.
    pub fn irqchip(&mut self, off: bool) -> &mut Self {
        self.irqchip_off = off;
        self
    }
}

impl<S> Run<S>
where
    S: AsRef<OsStr>,
{
    /// Spawns a QEMU instance running the given `image`.
    pub fn spawn(&self, image: &str) -> Result<Child> {
        let mut cmd = Command::new(self.cmd.as_ref());
        cmd.arg("-nographic")
            .arg("-drive")
            .arg(format!("file={}", image))
            .arg("-rtc")
            .arg("base=localtime")
            .arg("-net")
            .arg("nic,model=virtio")
            .arg("-m")
            .arg(format!("{}M", self.memory));
        if self.enable_kvm {
            cmd.arg("-enable-kvm");
        }
        if self.irqchip_off {
            cmd.arg("-machine").arg("kernel_irqchip=off");
        }
        if !self.forwards.is_empty() {
            let forwards = self.forwards.iter().map(|forward| {
                format!(
                    "{}={}::{}-:{}",
                    forward.direction, forward.protocol, forward.from, forward.to
                )
            });
            let arg = ["user".to_string()]
                .into_iter()
                .chain(forwards)
                .collect::<Vec<_>>()
                .join(",");
            cmd.arg("-net").arg(arg);
        }

        cmd.spawn()
    }
}
