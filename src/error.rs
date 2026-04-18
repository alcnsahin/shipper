use thiserror::Error;

/// Typed errors for the store-submission pipeline.
///
/// Variants are added incrementally as phases wire them in; this enum only
/// lists error shapes that are either live or scheduled for the next phase.
/// Speculative shapes are deliberately absent — `anyhow::anyhow!` covers
/// everything else until a real call-site earns a named variant.
#[derive(Debug, Error)]
pub(crate) enum ShipperError {
    /// Missing a required external binary (xcodebuild, apksigner, …).
    #[error("{tool} not found on PATH — {hint}")]
    ToolNotFound {
        tool: &'static str,
        /// Concrete remediation, e.g. "install Xcode from the App Store".
        hint: &'static str,
    },

    /// Store API returned a non-auth, non-success status. `message`
    /// carries the operation label and (when present) the server body.
    #[error("API error ({status}): {message}")]
    ApiError { status: u16, message: String },

    /// Store API returned `401`/`403`, or an upstream auth exchange
    /// (Google OAuth2, Apple JWT) failed.
    #[error("Auth error: {0}")]
    AuthError(String),

    /// Bundle/IPA upload failed at the HTTP boundary. Distinct from
    /// `ApiError` because the remediation is usually "retry later" or
    /// "verify artifact integrity", not "fix the API call".
    #[error("Upload failed: {0}")]
    UploadFailed(String),

    /// A build subprocess (`xcodebuild`, `gradle`, `expo prebuild`, `pod install`)
    /// exited with a non-zero status. The message carries extracted error lines.
    #[error("Build failed: {0}")]
    BuildFailed(String),
}
