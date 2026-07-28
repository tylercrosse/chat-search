use std::path::PathBuf;

/// Library crates define a concrete error enum so callers can match on the variants.
/// The `cs` binary wraps these in `anyhow` where it only needs to report them.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("config at {path} is not valid TOML: {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("could not serialise config: {0}")]
    ConfigWrite(#[from] toml::ser::Error),

    #[error("{path} is not valid JSON: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("archive at {root} holds {count} machine identities; expected exactly one")]
    AmbiguousMachine { root: PathBuf, count: usize },

    #[error("source id {0:?} is not a valid identifier: use lowercase letters, digits and dashes")]
    BadSourceId(String),

    #[error("duplicate source id {0:?}")]
    DuplicateSourceId(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Attach the offending path to an io error. `std::io::Error` alone never says *which*
/// file failed, which makes archiver bugs unnecessarily hard to diagnose.
pub(crate) trait IoContext<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::result::Result<T, std::io::Error> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io { path: path.into(), source })
    }
}
