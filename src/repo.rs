use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

pub struct RepositoryLock {
    _file: fs::File,
}

#[derive(Debug, Clone)]
pub struct Repository {
    pub root: PathBuf,
    pub metadata_dir: PathBuf,
}

impl Repository {
    pub fn for_init(root: Option<&Path>, metadata_name: &str) -> Result<Self> {
        validate_metadata_name(metadata_name)?;
        let root = match root {
            Some(path) => absolutize(path)?,
            None => env::current_dir().context("failed to read current directory")?,
        };
        Ok(Self {
            metadata_dir: root.join(metadata_name),
            root,
        })
    }

    pub fn discover(root: Option<&Path>, metadata_name: &str) -> Result<Self> {
        validate_metadata_name(metadata_name)?;
        let start = match root {
            Some(path) => absolutize(path)?,
            None => env::current_dir().context("failed to read current directory")?,
        };
        if root.is_some() {
            let metadata_dir = start.join(metadata_name);
            if !metadata_dir.is_dir() {
                bail!("{} does not contain {metadata_name}", start.display());
            }
            return Ok(Self {
                root: start,
                metadata_dir,
            });
        }

        for directory in start.ancestors() {
            let metadata_dir = directory.join(metadata_name);
            if metadata_dir.is_dir() {
                return Ok(Self {
                    root: directory.to_path_buf(),
                    metadata_dir,
                });
            }
        }
        bail!(
            "no {metadata_name} directory found from {}; run `strandmap init`",
            start.display()
        )
    }

    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    pub fn ensure_dir(&self, relative: &str) -> Result<PathBuf> {
        let path = self.metadata_dir.join(relative);
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        Ok(path)
    }

    pub fn lock(&self) -> Result<RepositoryLock> {
        fs::create_dir_all(&self.metadata_dir)
            .with_context(|| format!("failed to create {}", self.metadata_dir.display()))?;
        let path = self.metadata_dir.join(".lock");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        fs2::FileExt::lock_exclusive(&file)
            .with_context(|| format!("failed to lock {}", path.display()))?;
        Ok(RepositoryLock { _file: file })
    }
}

fn validate_metadata_name(name: &str) -> Result<()> {
    let path = Path::new(name);
    if name.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || name == "."
        || name == ".."
    {
        bail!("--metadata must be one directory name");
    }
    Ok(())
}

fn absolutize(path: &Path) -> Result<PathBuf> {
    let result = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .context("failed to read current directory")?
            .join(path)
    };
    result
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", result.display()))
}
