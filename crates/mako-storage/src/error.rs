use std::{error::Error, fmt};

/// Stable storage failure classes that do not expose adapter-specific details.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageErrorKind {
    InvalidRequest,
    Unsupported,
    Conflict,
    Unavailable,
    Timeout,
    Corruption,
    Io,
    Internal,
}

/// A safe storage error. Vendor errors are translated at the adapter boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageError {
    pub kind: StorageErrorKind,
    pub operation: &'static str,
    pub retryable: bool,
    pub message: String,
}

impl StorageError {
    #[must_use]
    pub fn new(
        kind: StorageErrorKind,
        operation: &'static str,
        retryable: bool,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            operation,
            retryable,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn invalid(operation: &'static str, message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::InvalidRequest, operation, false, message)
    }

    #[must_use]
    pub fn unsupported(operation: &'static str, message: impl Into<String>) -> Self {
        Self::new(StorageErrorKind::Unsupported, operation, false, message)
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "storage {:?} during {}: {}",
            self.kind, self.operation, self.message
        )
    }
}

impl Error for StorageError {}

pub type StorageResult<T> = Result<T, StorageError>;
