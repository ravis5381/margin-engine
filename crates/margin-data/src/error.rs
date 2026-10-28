use margin_domain::DomainError;

#[derive(Debug, thiserror::Error)]
pub enum DataError {
    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error("configuration error: {detail}")]
    Configuration { detail: String },

    #[error("download rejected: {detail}")]
    DownloadRejected { detail: String },

    #[error("source `{id}` is disabled: {blocker}")]
    SourceDisabled { id: String, blocker: String },

    #[error("archive rejected: {detail}")]
    ArchiveRejected { detail: String },

    #[error("database error: {detail}")]
    Database { detail: String },

    #[error("margin file was not found: {detail}")]
    MarginNotFound { detail: String },

    #[error("margin calculation rejected: {detail}")]
    MarginRejected { detail: String },
}
