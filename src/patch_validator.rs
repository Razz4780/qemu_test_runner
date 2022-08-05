use std::{
    collections::{hash_map::Entry, HashMap},
    ffi::OsStr,
    fmt::{self, Display, Formatter},
    io,
    ops::Not,
    path::{Path, PathBuf},
};
use tokio::fs;

#[derive(Debug)]
pub enum ValidationError {
    Io(io::Error),
    NoFilename,
    InvalidFilename,
    NotAFile,
    AlreadySeen(PathBuf),
}

impl Display for ValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::NoFilename => f.write_str("no filename"),
            Self::InvalidFilename => {
                f.write_str("invalid filename, expected format ab123456.patch")
            }
            Self::NotAFile => f.write_str("not a file"),
            Self::AlreadySeen(path) => write!(f, "id already seen before: {}", path.display()),
        }
    }
}

impl From<io::Error> for ValidationError {
    fn from(error: io::Error) -> Self {
        ValidationError::Io(error)
    }
}

#[derive(Debug)]
pub struct Patch {
    path: PathBuf,
}

impl Patch {
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn id(&self) -> &str {
        self.path
            .file_stem()
            .and_then(OsStr::to_str)
            .expect("this struct should contain only validated paths")
    }
}

#[derive(Default)]
pub struct PatchValidator {
    seen_patches: HashMap<String, PathBuf>,
}

impl PatchValidator {
    fn check_filename(filename: &str) -> bool {
        filename.is_ascii()
            && filename.len() == 14
            && filename.ends_with(".patch")
            && filename[..2].chars().all(|c| c.is_ascii_lowercase())
            && filename[2..8].chars().all(|c| c.is_ascii_digit())
    }

    pub async fn validate(&mut self, path: &Path) -> Result<Patch, ValidationError> {
        let filename = path
            .file_name()
            .ok_or(ValidationError::NoFilename)?
            .to_str()
            .ok_or(ValidationError::InvalidFilename)?;

        Self::check_filename(filename)
            .not()
            .then_some(Err(ValidationError::InvalidFilename))
            .unwrap_or(Ok(()))?;

        let metadata = fs::metadata(&path).await?;
        if !metadata.is_file() {
            return Err(ValidationError::NotAFile);
        }

        match self.seen_patches.entry(filename.to_string()) {
            Entry::Vacant(e) => {
                e.insert(path.to_path_buf());
            }
            Entry::Occupied(e) => return Err(ValidationError::AlreadySeen(e.get().clone())),
        }

        Ok(Patch {
            path: path.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case("asdf", false)]
    #[test_case("", false)]
    #[test_case("ab123456.patcc", false)]
    #[test_case("11111111.patch", false)]
    #[test_case("ab1234567.patch", false)]
    #[test_case("ab123456.patch", true)]
    fn check_filename(filename: &str, expected: bool) {
        assert_eq!(PatchValidator::check_filename(filename), expected)
    }

    #[tokio::test]
    async fn validate() {
        let tmp = tempfile::tempdir().unwrap();

        let mut validator = PatchValidator::default();

        let dir_path = tmp.path().join("aa111111.patch");
        fs::create_dir(&dir_path).await.unwrap();
        validator
            .validate(&dir_path)
            .await
            .expect_err("directory should not pass");

        validator
            .validate(&tmp.path().join("aa222222.patch"))
            .await
            .expect_err("non-existent path should not pass");

        validator
            .validate("/".as_ref())
            .await
            .expect_err("no filename should not pass");

        let file_path = tmp.path().join("aa333333.pat");
        fs::write(&file_path, &[]).await.unwrap();
        validator
            .validate(&file_path)
            .await
            .expect_err("invalid filename should not pass");

        let file_1_path = tmp.path().join("aa444444.patch");
        fs::write(&file_1_path, &[]).await.unwrap();
        let dir = tmp.path().join("dir");
        fs::create_dir(&dir).await.unwrap();
        let file_2_path = dir.join("aa444444.patch");
        fs::write(&file_2_path, &[]).await.unwrap();
        let patch = validator
            .validate(&file_1_path)
            .await
            .expect("valid path should pass");
        assert_eq!(patch.path(), file_1_path.as_path());
        assert_eq!(patch.id(), "aa444444");
        let error = validator
            .validate(&file_2_path)
            .await
            .expect_err("duplicate id should not pass");
        assert!(matches!(error, ValidationError::AlreadySeen(p) if p == file_1_path));
    }
}
