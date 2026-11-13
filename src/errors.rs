//! Application error type.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Errors that can terminate a RustTemp command.
///
/// Individual sensor read failures are *not* errors: they are represented as
/// [`crate::sensors::Reading::Unavailable`] so that one broken sensor never
/// hides the others.
#[derive(Debug)]
pub enum Error {
    /// No sensor backend exists for the current operating system.
    UnsupportedPlatform(&'static str),
    /// A filesystem operation needed for sensor discovery failed.
    Io { path: PathBuf, source: io::Error },
    /// An operating-system API used for sensor access failed.
    Platform(String),
    /// The user supplied an invalid configuration (exit code 2).
    InvalidConfig(String),
    /// Writing to stdout failed (for example, a closed pipe).
    Output(io::Error),
    /// JSON serialization failed.
    Json(serde_json::Error),
}

impl Error {
    /// Process exit code for this error.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::InvalidConfig(_) => 2,
            _ => 1,
        }
    }

    /// True when stdout was closed by the reader (e.g. `rusttemp --json | head -1`).
    pub fn is_broken_pipe(&self) -> bool {
        matches!(self, Error::Output(e) if e.kind() == io::ErrorKind::BrokenPipe)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnsupportedPlatform(os) => write!(
                f,
                "no sensor backend is implemented for this operating system ({os}); \
                 supported: Linux (sysfs hwmon/thermal) and Windows (ACPI thermal zones)"
            ),
            Error::Io { path, source } => write!(f, "cannot access {}: {source}", path.display()),
            Error::Platform(msg) => write!(f, "{msg}"),
            Error::InvalidConfig(msg) => write!(f, "{msg}"),
            Error::Output(e) => write!(f, "cannot write output: {e}"),
            Error::Json(e) => write!(f, "cannot serialize JSON: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::Output(e) => Some(e),
            Error::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
