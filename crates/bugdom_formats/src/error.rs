use std::path::PathBuf;

/// Errors from parsing the original data files.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("malformed data")]
    Binary(#[from] binrw::Error),

    /// An I/O error while seeking or reading inside already-loaded data.
    #[error("malformed data")]
    Stream(#[from] std::io::Error),

    /// The data is structurally valid but uses something we do not support,
    /// or contradicts itself (e.g. an offset past the end of the file).
    #[error("{0}")]
    Invalid(String),

    /// Adds the name of what was being parsed, e.g. a file or resource.
    #[error("{context}")]
    Context {
        context: String,
        #[source]
        source: Box<Error>,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

/// Attaches context to parse errors, like `anyhow::Context`.
pub trait ResultExt<T> {
    fn context(self, context: impl FnOnce() -> String) -> Result<T>;
}

impl<T, E: Into<Error>> ResultExt<T> for Result<T, E> {
    fn context(self, context: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|source| Error::Context {
            context: context(),
            source: Box::new(source.into()),
        })
    }
}

/// Reads a whole file, reporting the path on failure.
pub(crate) fn read_file(path: &std::path::Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })
}
