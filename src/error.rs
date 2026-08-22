use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),
    #[error("unsupported operating system: {0}")]
    UnsupportedPlatform(&'static str),
    #[error("the operating system version does not support the requested audio capture mode")]
    UnsupportedOsVersion,
    #[error("audio capture permission was denied")]
    PermissionDenied,
    #[error("target process {0} has no active audio object")]
    ProcessNotFound(u32),
    #[error("native audio operation failed: {0}")]
    Native(String),
    #[error("audio conversion failed: {0}")]
    AudioConversion(String),
    #[error("FunASR network operation failed: {0}")]
    Network(String),
    #[error("invalid FunASR protocol message: {0}")]
    Protocol(String),
    #[error("FunASR audio queue is full; audio continuity was lost")]
    AudioQueueFull,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
