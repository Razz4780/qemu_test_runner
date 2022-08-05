use std::{
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};
use tempfile::TempDir;
use tokio::fs;

pub enum MaybeTmp {
    Tmp(TempDir),
    NotTmp(PathBuf),
}

impl MaybeTmp {
    pub async fn at_path(path: &Path) -> io::Result<Self> {
        let path = fs::canonicalize(path).await?;

        if let Err(e) = fs::create_dir_all(&path).await {
            if e.kind() != ErrorKind::AlreadyExists {
                return Err(e);
            }
        }

        Ok(Self::NotTmp(path))
    }

    pub fn tmp() -> io::Result<Self> {
        let dir = tempfile::tempdir()?;
        Ok(Self::Tmp(dir))
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Tmp(tmp) => tmp.path(),
            Self::NotTmp(path) => path.as_path(),
        }
    }
}
