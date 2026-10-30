use std::{fmt, io};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Database(rusqlite::Error),
    Configuration(String),
    Http(String),
    ReadBudgetExceeded { kind: &'static str, limit: usize },
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Database(e) => write!(f, "database error: {e}"),
            Self::Configuration(e) => write!(f, "configuration error: {e}"),
            Self::Http(e) => write!(f, "HTTP error: {e}"),
            Self::ReadBudgetExceeded { kind, limit } => {
                write!(f, "read budget exceeded ({kind}, limit {limit})")
            }
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e)
    }
}
pub type Result<T> = std::result::Result<T, Error>;
