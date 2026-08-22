#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub(crate) use macos::{
    PlatformSession, list_audio_applications, list_output_devices, process_isolation_supported,
    start,
};
#[cfg(windows)]
pub(crate) use windows::{
    PlatformSession, list_audio_applications, list_output_devices, process_isolation_supported,
    start,
};

#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported {
    use std::sync::mpsc::Receiver;

    use crate::{
        AudioApplication, AudioFrame, CaptureConfig, CaptureMode, Error, OutputDevice, Result,
    };

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

    pub(crate) fn list_audio_applications() -> Result<Vec<AudioApplication>> {
        Err(Error::UnsupportedPlatform(
            "audio application discovery is available only on Windows and macOS",
        ))
    }

    pub(crate) fn process_isolation_supported() -> Result<bool> {
        Err(Error::UnsupportedPlatform(
            "process audio capture is available only on Windows and macOS",
        ))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) use unsupported::{
    PlatformSession, list_audio_applications, list_output_devices, process_isolation_supported,
    start,
};
