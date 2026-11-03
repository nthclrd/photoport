use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("usbmuxd service unreachable (Apple driver missing or not running)")]
    UsbmuxdUnavailable,

    #[error("no iPhone detected over USB")]
    NoDevice,

    #[error("iPhone not trusted: unlock it and tap 'Trust'")]
    NotPaired,

    #[error("cancelled")]
    Cancelled,

    #[error("idevice error: {0}")]
    Idevice(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl From<idevice::IdeviceError> for Error {
    fn from(e: idevice::IdeviceError) -> Self {
        let msg = format!("{e}");
        let low = msg.to_lowercase();
        if low.contains("pair") || low.contains("trust") || low.contains("invalidhostid") {
            return Error::NotPaired;
        }
        Error::Idevice(msg)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
