#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub(crate) use macos::{PlatformSession, list_output_devices, start};
#[cfg(windows)]
pub(crate) use windows::{PlatformSession, list_output_devices, start};

#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported {
    use std::sync::mpsc::Receiver;

    use crate::{AudioFrame, CaptureConfig, CaptureMode, Error, OutputDevice, Result};

    pub(crate) struct PlatformSession;

    impl PlatformSession {
        pub(crate) fn stop(&mut self) -> Result<()> {
            Ok(())
        }
    }

    pub(crate) fn start(
        _: CaptureConfig,
    ) -> Result<(PlatformSession, Receiver<AudioFrame>, CaptureMode)> {
        Err(Error::UnsupportedPlatform(
            "vapor-tap supports Windows 10+ and macOS 14.2+",
        ))
    }

    pub(crate) fn list_output_devices() -> Result<Vec<OutputDevice>> {
        Err(Error::UnsupportedPlatform(
            "output endpoint enumeration is available only on Windows",
        ))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) use unsupported::{PlatformSession, list_output_devices, start};
