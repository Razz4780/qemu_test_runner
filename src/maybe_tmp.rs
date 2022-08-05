use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};
use tempfile::TempDir;
use tokio::fs;

pub enum MaybeTmp {
    Tmp(TempDir),
    NotTmp(PathBuf),
}

impl MaybeTmp {
    pub async fn at_path(path: PathBuf) -> Self {
        if let Err(e) = fs::create_dir_all(&path).await {
            if e.kind() != ErrorKind::AlreadyExists {
                panic!("failed to access directory {}: {}", path.display(), e);
            }
        }

        Self::NotTmp(path)
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Tmp(tmp) => tmp.path(),
            Self::NotTmp(path) => path.as_path(),
        }
    }
}

impl Default for MaybeTmp {
    fn default() -> Self {
        let dir = tempfile::tempdir().expect("failed to create a temporary directory");
        Self::Tmp(dir)
    }
}
