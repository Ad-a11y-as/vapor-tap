use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use flexaudio_core::backend::{CaptureBackend, RawSink};
use flexaudio_core::raw_ring;
use flexaudio_core::types::ProcessMode;
use flexaudio_os_windows::{WasapiProcessBackend, WasapiSystemBackend};

use crate::{
    AudioFormat, AudioFrame, CaptureConfig, CaptureMode, CaptureSource, Error, OutputDevice, Result,
};

pub(crate) struct PlatformSession {
    backend: Box<dyn CaptureBackend>,
    stop: Arc<AtomicBool>,
    drain: Option<JoinHandle<()>>,
    stopped: bool,
}

pub(crate) fn start(
    config: CaptureConfig,
) -> Result<(PlatformSession, mpsc::Receiver<AudioFrame>, CaptureMode)> {
    let (mut backend, mode): (Box<dyn CaptureBackend>, CaptureMode) = match config.source {
        CaptureSource::Process { pid } if windows_build()? >= 20_348 => (
            Box::new(WasapiProcessBackend::new(pid, ProcessMode::Include)),
            CaptureMode::ProcessLoopback,
        ),
        CaptureSource::Process { .. } => (
            Box::new(WasapiSystemBackend::new(false, None)),
            CaptureMode::OutputLoopback,
        ),
        CaptureSource::OutputDevice { name } => (
            Box::new(WasapiSystemBackend::new(false, name)),
            CaptureMode::OutputLoopback,
        ),
    };
    let (sample_rate, channels) = backend.native_format();
    let format = AudioFormat {
        sample_rate,
        channels,
    };
    let ring_samples = sample_rate as usize * channels as usize * 2;
    let (producer, mut consumer) = raw_ring(ring_samples);
    let sink = RawSink::new(producer, sample_rate, channels);
    backend
        .start(sink)
        .map_err(|error| Error::Native(error.to_string()))?;

    let (sender, receiver) = mpsc::sync_channel(config.channel_capacity);
    let stop = Arc::new(AtomicBool::new(false));
    let drain_stop = Arc::clone(&stop);
    let drain = thread::Builder::new()
        .name("vapor-tap-pcm-drain".into())
        .spawn(move || {
            let packet_samples = (sample_rate as usize / 50).max(1) * usize::from(channels);
            let mut samples = vec![0.0; packet_samples];
            while !drain_stop.load(Ordering::Acquire) {
                if consumer.available() < packet_samples {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                let count = consumer.pop_slice(&mut samples);
                if count == packet_samples {
                    let frame = AudioFrame {
                        format,
                        samples: samples.clone(),
                    };
                    let _ = sender.try_send(frame);
                }
            }
        })
        .map_err(Error::Io)?;

    Ok((
        PlatformSession {
            backend,
            stop,
            drain: Some(drain),
            stopped: false,
        },
        receiver,
        mode,
    ))
}

#[repr(C)]
struct RtlOsVersionInfo {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform_id: u32,
    service_pack: [u16; 128],
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetVersion(version: *mut RtlOsVersionInfo) -> i32;
}

fn windows_build() -> Result<u32> {
    let mut version = RtlOsVersionInfo {
        size: size_of::<RtlOsVersionInfo>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform_id: 0,
        service_pack: [0; 128],
    };
    // RtlGetVersion is used because GetVersionEx can return a manifest-dependent value.
    let status = unsafe { RtlGetVersion(&mut version) };
    if status < 0 {
        return Err(Error::Native(format!(
            "RtlGetVersion failed with NTSTATUS {status:#x}"
        )));
    }
    if version.major < 10 {
        return Err(Error::UnsupportedOsVersion);
    }
    Ok(version.build)
}

pub(crate) fn list_output_devices() -> Result<Vec<OutputDevice>> {
    flexaudio_os_windows::list_output_devices()
        .map(|devices| {
            devices
                .into_iter()
                .map(|device| OutputDevice {
                    name: device.name,
                    sample_rate: device.sample_rate,
                    channels: device.channels,
                    is_default: device.is_default,
                })
                .collect()
        })
        .map_err(|error| Error::Native(error.to_string()))
}

impl PlatformSession {
    pub(crate) fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.backend.stop();
        self.stop.store(true, Ordering::Release);
        if let Some(drain) = self.drain.take() {
            drain
                .join()
                .map_err(|_| Error::Native("PCM drain thread panicked".into()))?;
        }
        self.stopped = true;
        Ok(())
    }
}
