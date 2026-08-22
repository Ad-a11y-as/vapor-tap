use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use flexaudio_core::backend::{CaptureBackend, RawSink};
use flexaudio_core::raw_ring;
use flexaudio_core::types::ProcessMode;
use flexaudio_os_windows::WasapiProcessBackend;

use crate::{AudioFormat, AudioFrame, CaptureConfig, Error, Result};

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const PACKET_FRAMES: usize = 960; // 20 ms

pub(crate) struct PlatformSession {
    backend: WasapiProcessBackend,
    stop: Arc<AtomicBool>,
    drain: Option<JoinHandle<()>>,
    stopped: bool,
}

pub(crate) fn start(
    config: CaptureConfig,
) -> Result<(PlatformSession, mpsc::Receiver<AudioFrame>)> {
    ensure_supported_version()?;
    let format = AudioFormat {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
    };
    let ring_samples = SAMPLE_RATE as usize * CHANNELS as usize * 2;
    let (producer, mut consumer) = raw_ring(ring_samples);
    let sink = RawSink::new(producer, SAMPLE_RATE, CHANNELS);
    let mut backend = WasapiProcessBackend::new(config.pid, ProcessMode::Include);
    backend
        .start(sink)
        .map_err(|error| Error::Native(error.to_string()))?;

    let (sender, receiver) = mpsc::sync_channel(config.channel_capacity);
    let stop = Arc::new(AtomicBool::new(false));
    let drain_stop = Arc::clone(&stop);
    let drain = thread::Builder::new()
        .name("vapor-tap-pcm-drain".into())
        .spawn(move || {
            let packet_samples = PACKET_FRAMES * CHANNELS as usize;
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

fn ensure_supported_version() -> Result<()> {
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
    if version.build < 20_348 {
        return Err(Error::UnsupportedOsVersion);
    }
    Ok(())
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
