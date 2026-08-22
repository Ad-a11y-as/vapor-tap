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

/// An application with a currently running audio output stream.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioApplication {
    pub pid: u32,
    /// Human-readable process or application name.
    pub name: String,
    /// Executable path on Windows or bundle identifier on macOS, when known.
    pub identifier: Option<String>,
    /// Windows output endpoint names used by this application, when known.
    pub output_devices: Vec<String>,
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

/// Lists applications that currently have a running output audio stream.
/// Start playback before calling this function so the application is visible.
pub fn list_audio_applications() -> Result<Vec<AudioApplication>> {
    platform::list_audio_applications()
}

/// Returns whether this operating system can isolate capture by process.
/// Windows 10 returns `false`; Windows build 20348+ and macOS 14.2+ return
/// `true`.
pub fn process_isolation_supported() -> Result<bool> {
    platform::process_isolation_supported()
}

/// Resolves a PID, exact application name/identifier, or unique substring to
/// one currently running audio application.
pub fn resolve_audio_application(query: &str) -> Result<AudioApplication> {
    let applications = list_audio_applications()?;
    resolve_audio_application_from(query, &applications)
}

fn resolve_audio_application_from(
    query: &str,
    applications: &[AudioApplication],
) -> Result<AudioApplication> {
    let query = query.trim();
    if query.is_empty() {
        return Err(Error::InvalidArgument(
            "application selector must not be empty",
        ));
    }
    if let Ok(pid) = query.parse::<u32>()
        && let Some(application) = applications
            .iter()
            .find(|application| application.pid == pid)
    {
        return Ok(application.clone());
    }

    let query_lower = query.to_lowercase();
    let matches_query = |application: &&AudioApplication, exact: bool| {
        let name = application.name.to_lowercase();
        let identifier = application
            .identifier
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        if exact {
            name == query_lower || identifier == query_lower
        } else {
            name.contains(&query_lower) || identifier.contains(&query_lower)
        }
    };
    let exact: Vec<_> = applications
        .iter()
        .filter(|app| matches_query(app, true))
        .collect();
    let matches = if exact.is_empty() {
        applications
            .iter()
            .filter(|app| matches_query(app, false))
            .collect::<Vec<_>>()
    } else {
        exact
    };
    match matches.as_slice() {
        [application] => Ok((*application).clone()),
        [] => Err(Error::ApplicationNotFound(query.into())),
        many => Err(Error::ApplicationAmbiguous {
            query: query.into(),
            matches: many
                .iter()
                .map(|application| format!("{} (PID {})", application.name, application.pid))
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
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

    #[test]
    fn application_model_can_describe_an_output_session() {
        let application = AudioApplication {
            pid: 42,
            name: "player.exe".into(),
            identifier: Some("C:\\Apps\\player.exe".into()),
            output_devices: vec!["Speakers".into()],
        };
        assert_eq!(application.pid, 42);
        assert_eq!(application.name, "player.exe");
    }

    #[test]
    fn application_selector_supports_exact_and_unique_substring_matches() {
        let applications = vec![
            AudioApplication {
                pid: 10,
                name: "WeChat.exe".into(),
                identifier: Some("C:\\Apps\\WeChat.exe".into()),
                output_devices: vec!["Speakers".into()],
            },
            AudioApplication {
                pid: 20,
                name: "chrome.exe".into(),
                identifier: Some("C:\\Apps\\chrome.exe".into()),
                output_devices: vec!["Speakers".into()],
            },
        ];
        assert_eq!(
            resolve_audio_application_from("wechat", &applications)
                .unwrap()
                .pid,
            10
        );
        assert_eq!(
            resolve_audio_application_from("CHROME.EXE", &applications)
                .unwrap()
                .pid,
            20
        );
        assert_eq!(
            resolve_audio_application_from("20", &applications)
                .unwrap()
                .pid,
            20
        );
    }

    #[test]
    fn application_selector_reports_ambiguity() {
        let applications = vec![
            AudioApplication {
                pid: 10,
                name: "chrome.exe".into(),
                identifier: None,
                output_devices: vec![],
            },
            AudioApplication {
                pid: 20,
                name: "chrome.exe".into(),
                identifier: None,
                output_devices: vec![],
            },
        ];
        assert!(matches!(
            resolve_audio_application_from("chrome", &applications),
            Err(Error::ApplicationAmbiguous { .. })
        ));
    }
}
