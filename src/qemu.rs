use std::{
    ffi::OsStr,
    io::Result,
    process::{Command, Output},
};

/// A command for building QEMU images.
pub struct Build<S> {
    cmd: S,
}

impl<S> Build<S> {
    /// Creates a new instance of this struct.
    /// The given `cmd` will be invoked when creating an image.
    pub fn new(cmd: S) -> Self {
        Self { cmd }
    }
}

impl Default for Build<&'static str> {
    /// Creates a new instance of this struct.
    /// The instance will use `qemu-img` to create images.
    fn default() -> Self {
        Self { cmd: "qemu-img" }
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
