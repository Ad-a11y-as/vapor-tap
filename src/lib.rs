//! Cross-platform, per-process audio capture.
//!
//! The platform backends use WASAPI process loopback on Windows 11 and Core
//! Audio process taps on macOS 14.2 or newer. Captured samples are interleaved
//! 32-bit floating-point PCM.

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

/// Configures a process capture session.
#[derive(Clone, Copy, Debug)]
pub struct CaptureConfig {
    /// Target process identifier. Child processes are included on Windows.
    pub pid: u32,
    /// Capacity of the bounded packet channel between the realtime capture
    /// thread and the consumer. New packets are dropped when it is full.
    pub channel_capacity: usize,
}

impl CaptureConfig {
    pub fn for_pid(pid: u32) -> Self {
        Self {
            pid,
            channel_capacity: 64,
        }
    }
}

/// A running native capture session.
pub struct CaptureSession {
    inner: platform::PlatformSession,
    receiver: Receiver<AudioFrame>,
}

impl CaptureSession {
    pub fn start(config: CaptureConfig) -> Result<Self> {
        if config.pid == 0 {
            return Err(Error::InvalidArgument("pid must be non-zero"));
        }
        if config.channel_capacity == 0 {
            return Err(Error::InvalidArgument("channel_capacity must be non-zero"));
        }

        let (inner, receiver) = platform::start(config)?;
        Ok(Self { inner, receiver })
    }

    pub fn frames(&self) -> &Receiver<AudioFrame> {
        &self.receiver
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
}
