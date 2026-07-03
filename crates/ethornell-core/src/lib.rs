use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, EthornellError>;

#[derive(Debug, thiserror::Error)]
pub enum EthornellError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid game root: {0}")]
    InvalidGameRoot(PathBuf),
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRoot {
    path: PathBuf,
}

impl GameRoot {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if !path.is_dir() {
            return Err(EthornellError::InvalidGameRoot(path));
        }
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourcePath(String);

impl ResourcePath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into().replace('\\', "/"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Cp932,
    Utf8,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub engine: Option<String>,
    pub compatibility: Option<String>,
}

pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ethornell=info,wgpu=warn".into()),
        )
        .try_init();
}
