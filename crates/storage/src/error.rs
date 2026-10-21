//! Error type.

use core::fmt;

/// Storage errors.
#[derive(Debug)]
pub enum StorageError {
    /// The database key is wrong, or the file is not an OutpostWave database.
    WrongKeyOrCorrupted,
    /// The database was created by a newer version of OutpostWave.
    TooNew {
        /// Schema version found in the file.
        found: u32,
        /// Latest version known by this build.
        supported: u32,
    },
    /// The OS random number generator failed.
    Rng,
    /// A stored value has an unexpected format.
    InvalidData(&'static str),
    /// OS credential store error.
    #[cfg(feature = "os-keystore")]
    Keystore(String),
    /// Underlying SQLite error.
    Sqlite(rusqlite::Error),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongKeyOrCorrupted => f.write_str("wrong database key or corrupted database"),
            Self::TooNew { found, supported } => write!(
                f,
                "database schema v{found} is newer than supported v{supported}; update OutpostWave"
            ),
            Self::Rng => f.write_str("system random number generator failed"),
            Self::InvalidData(what) => write!(f, "invalid stored data: {what}"),
            #[cfg(feature = "os-keystore")]
            Self::Keystore(e) => write!(f, "credential store error: {e}"),
            Self::Sqlite(e) => write!(f, "sqlite error: {e}"),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(e: rusqlite::Error) -> Self {
        match e.sqlite_error_code() {
            Some(rusqlite::ErrorCode::NotADatabase) => Self::WrongKeyOrCorrupted,
            _ => Self::Sqlite(e),
        }
    }
}
