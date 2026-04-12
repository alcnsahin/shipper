use thiserror::Error;

#[derive(Debug, Error)]
pub enum ShipperError {
    #[error("Config not found: {0}")]
    ConfigNotFound(String),

    #[error("Missing required field in config: {0}")]
    MissingConfig(String),

    #[error("Tool not found: {0} — install it and try again")]
    ToolNotFound(String),

    #[error("Build failed: {0}")]
    BuildFailed(String),

    #[error("Upload failed: {0}")]
    UploadFailed(String),

    #[error("API error ({status}): {message}")]
    ApiError { status: u16, message: String },

    #[error("Auth error: {0}")]
    AuthError(String),

    #[error("Timeout waiting for: {0}")]
    Timeout(String),
}
