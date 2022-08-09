use std::{
    io,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

use crate::prepare_dir;

/// A wrapper over a directory that may or may not be temporary.
pub enum MaybeTmp {
    /// A temporary directory which will be removed when this struct is dropped.
    Tmp(TempDir),
    /// A regular directory.
    NotTmp(PathBuf),
}

impl MaybeTmp {
    /// Creates a new instance of this struct, wrapping a non-temporary directory.
    /// If the directory does not exist, it will be created.
    /// # Arguments
    /// * path - path of the directory to wrap
    /// # Returns
    /// A new instance of this struct.
    pub async fn at_path(path: PathBuf) -> io::Result<Self> {
        prepare_dir(path.as_path()).await?;
        Ok(Self::NotTmp(path))
    }

    /// # Returns
    /// A new instance of this struct, wrapping a new temporary directory.
    pub fn tmp() -> io::Result<Self> {
        let dir = tempfile::tempdir()?;
        Ok(Self::Tmp(dir))
    }

    /// # Returns
    /// The path to the wrapped directory.
    pub fn path(&self) -> &Path {
        match self {
            Self::Tmp(tmp) => tmp.path(),
            Self::NotTmp(path) => path.as_path(),
        }
    }
}
