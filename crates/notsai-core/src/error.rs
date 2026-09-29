//! The single error type shared across the notsAI domain.

/// Errors produced by the notsAI core crates.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The provided input was invalid.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// A requested item could not be found.
    #[error("not found: {0}")]
    NotFound(String),

    /// Transcription failed.
    #[error("transcription failed: {0}")]
    Transcription(String),

    /// An AI provider request failed.
    #[error("AI request failed: {0}")]
    AiRequest(String),

    /// A storage operation failed.
    #[error("storage error: {0}")]
    Storage(String),

    /// An underlying I/O error occurred.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// An internal invariant was violated.
    #[error("internal error: {0}")]
    Internal(String),
}

impl CoreError {
    /// Build an [`CoreError::InvalidInput`].
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }

    /// Build a [`CoreError::NotFound`].
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    /// Build a [`CoreError::Transcription`].
    pub fn transcription(msg: impl Into<String>) -> Self {
        Self::Transcription(msg.into())
    }

    /// Build a [`CoreError::AiRequest`].
    pub fn ai_request(msg: impl Into<String>) -> Self {
        Self::AiRequest(msg.into())
    }

    /// Build a [`CoreError::Storage`].
    pub fn storage(msg: impl Into<String>) -> Self {
        Self::Storage(msg.into())
    }

    /// Build a [`CoreError::Internal`].
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_build_distinct_variants() {
        assert!(matches!(
            CoreError::invalid_input("x"),
            CoreError::InvalidInput(_)
        ));
        assert!(matches!(CoreError::not_found("x"), CoreError::NotFound(_)));
        assert!(matches!(
            CoreError::transcription("x"),
            CoreError::Transcription(_)
        ));
        assert!(matches!(
            CoreError::ai_request("x"),
            CoreError::AiRequest(_)
        ));
        assert!(matches!(CoreError::storage("x"), CoreError::Storage(_)));
        assert!(matches!(CoreError::internal("x"), CoreError::Internal(_)));
    }

    #[test]
    fn io_and_json_errors_convert() {
        let io: CoreError = std::io::Error::other("disk").into();
        assert!(matches!(io, CoreError::Io(_)));

        let bad: serde_json::Error = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let json: CoreError = bad.into();
        assert!(matches!(json, CoreError::Json(_)));
    }
}
