//! Cross-platform application audio capture.
//!
//! The platform backends use WASAPI process loopback on Windows 11 and Core
//! Audio process taps on macOS 14.2 or newer. Windows 10 is supported by
//! capturing the default output endpoint mix when PID isolation is unavailable.
//! Captured samples are interleaved 32-bit floating-point PCM.

pub mod asr;
pub mod audio;
mod error;
mod platform;
mod wav;

pub use error::{Error, Result};
pub use wav::WavWriter;

use std::sync::mpsc::Receiver;

/// Audio format shared by all frames in a capture session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// One packet of interleaved floating-point PCM samples.
#[derive(Clone, Debug)]
pub struct AudioFrame {
    pub format: AudioFormat,
    pub samples: Vec<f32>,
}

impl AudioFrame {
    pub fn frame_count(&self) -> usize {
        self.samples.len() / usize::from(self.format.channels)
    }
}

/// Selects the native audio source for a capture session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureSource {
    /// Audio rendered by a process tree. On Windows 10 this automatically
    /// falls back to the complete default output mix and the PID is ignored.
    Process { pid: u32 },
    /// The complete mix rendered to one Windows output endpoint. `None` means
    /// the current default output endpoint. This works on Windows 10 and later.
    OutputDevice { name: Option<String> },
}

/// An active Windows output endpoint that can be captured through loopback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputDevice {
    /// Endpoint selector accepted by [`CaptureConfig::for_output_device`].
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub is_default: bool,
}

/// Native mechanism selected for a running capture session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureMode {
    /// The operating system isolates one process tree.
    ProcessLoopback,
    /// The complete mix sent to one output endpoint is captured.
    OutputLoopback,
}

/// Configures an audio capture session.
#[derive(Clone, Debug)]
pub struct CaptureConfig {
    pub source: CaptureSource,
    /// Capacity of the bounded packet channel between the realtime capture
    /// thread and the consumer. New packets are dropped when it is full.
    pub channel_capacity: usize,
}

impl CaptureConfig {
    pub fn for_pid(pid: u32) -> Self {
        Self {
            source: CaptureSource::Process { pid },
            channel_capacity: 64,
        }
    }

    /// Captures the complete mix sent to the named Windows output endpoint.
    /// This can capture any physical or virtual Windows render endpoint.
    pub fn for_output_device(name: impl Into<String>) -> Self {
        Self {
            source: CaptureSource::OutputDevice {
                name: Some(name.into()),
            },
            channel_capacity: 64,
        }
    }

    /// Captures the complete mix sent to the current default Windows output.
    pub fn for_default_output() -> Self {
        Self {
            source: CaptureSource::OutputDevice { name: None },
            channel_capacity: 64,
        }
    }
}

/// Lists active Windows render endpoints available for output-loopback capture.
pub fn list_output_devices() -> Result<Vec<OutputDevice>> {
    platform::list_output_devices()
}

/// A running native capture session.
pub struct CaptureSession {
    inner: platform::PlatformSession,
    receiver: Receiver<AudioFrame>,
    mode: CaptureMode,
}

impl CaptureSession {
    pub fn start(config: CaptureConfig) -> Result<Self> {
        match &config.source {
            CaptureSource::Process { pid: 0 } => {
                return Err(Error::InvalidArgument("pid must be non-zero"));
            }
            CaptureSource::OutputDevice { name: Some(name) } if name.trim().is_empty() => {
                return Err(Error::InvalidArgument(
                    "output device name must not be empty",
                ));
            }
            _ => {}
        }
        if config.channel_capacity == 0 {
            return Err(Error::InvalidArgument("channel_capacity must be non-zero"));
        }

        let (inner, receiver, mode) = platform::start(config)?;
        Ok(Self {
            inner,
            receiver,
            mode,
        })
    }

    pub fn frames(&self) -> &Receiver<AudioFrame> {
        &self.receiver
    }

    /// Reports whether the session is truly process-isolated or captures an
    /// output endpoint. A PID request automatically uses output loopback on
    /// Windows 10 for compatibility.
    pub fn mode(&self) -> CaptureMode {
        self.mode
    }

    pub fn stop(&mut self) -> Result<()> {
        self.inner.stop()
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        let _ = self.inner.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_count_uses_channel_count() {
        let frame = AudioFrame {
            format: AudioFormat {
                sample_rate: 48_000,
                channels: 2,
            },
            samples: vec![0.0; 20],
        };
        assert_eq!(frame.frame_count(), 10);
    }

    #[test]
    fn constructors_select_the_expected_source() {
        assert_eq!(
            CaptureConfig::for_pid(42).source,
            CaptureSource::Process { pid: 42 }
        );
        assert_eq!(
            CaptureConfig::for_output_device("Vapor Tap").source,
            CaptureSource::OutputDevice {
                name: Some("Vapor Tap".into())
            }
        );
        assert_eq!(
            CaptureConfig::for_default_output().source,
            CaptureSource::OutputDevice { name: None }
        );
    }
}
