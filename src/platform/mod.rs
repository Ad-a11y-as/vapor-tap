#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub(crate) use macos::{PlatformSession, start};
#[cfg(windows)]
pub(crate) use windows::{PlatformSession, start};

#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported {
    use std::sync::mpsc::Receiver;

    use crate::{AudioFrame, CaptureConfig, Error, Result};

    pub(crate) struct PlatformSession;

    impl PlatformSession {
        pub(crate) fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    pub(crate) fn start(_: CaptureConfig) -> Result<(PlatformSession, Receiver<AudioFrame>)> {
        Err(Error::UnsupportedPlatform(
            "vapor-tap supports only Windows 11 and macOS 14.2+",
        ))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) use unsupported::{PlatformSession, start};
