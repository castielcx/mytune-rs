use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("registry {op} failed for {path}: Win32 error {code}")]
    Registry { op: &'static str, path: String, code: u32 },
    #[error("unsupported registry value type {0}")]
    UnsupportedKind(u32),
    #[error("NVAPI {func} failed with status {status}")]
    Nvapi { func: &'static str, status: i32 },
    #[error("OS call failed: {0}")]
    Os(String),
    #[error("bad catalog data: {0}")]
    Catalog(String),
    #[error("bad backup entry: {0}")]
    BadBackup(String),
    /// Some values could not be restored. The backup is kept so revert can be retried.
    #[error("revert incomplete, {} failure(s): {}", .0.len(), .0.join("; "))]
    Revert(Vec<String>),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("`{0}` timed out")]
    Timeout(String),
    #[error("`{cmd}` exited with {code}: {output}")]
    ProcessFailed { cmd: String, code: i32, output: String },
}

pub type Result<T> = std::result::Result<T, Error>;
