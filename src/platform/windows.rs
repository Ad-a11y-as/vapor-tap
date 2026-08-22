use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use flexaudio_core::backend::{CaptureBackend, RawSink};
use flexaudio_core::raw_ring;
use flexaudio_core::types::ProcessMode;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, DEVICE_STATE_ACTIVE, IAudioSessionControl, IAudioSessionControl2,
    IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, STGM_READ,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::{Interface, PWSTR};

use crate::{
    AudioApplication, AudioFormat, AudioFrame, CaptureConfig, CaptureMode, CaptureSource, Error,
    OutputDevice, Result,
};

#[path = "windows_backend/mod.rs"]
mod windows_backend;
use windows_backend::{WasapiProcessBackend, WasapiSystemBackend};

pub(crate) struct PlatformSession {
    backend: Box<dyn CaptureBackend>,
    stop: Arc<AtomicBool>,
    drain: Option<JoinHandle<()>>,
    stopped: bool,
}

pub(crate) fn start(
    config: CaptureConfig,
) -> Result<(
    PlatformSession,
    mpsc::Receiver<AudioFrame>,
    mpsc::Receiver<Error>,
    CaptureMode,
)> {
    let (mut backend, backend_terminated, mode): (
        Box<dyn CaptureBackend>,
        Arc<AtomicBool>,
        CaptureMode,
    ) = match config.source {
        CaptureSource::Process { pid } if windows_build()? >= 20_348 => {
            let backend = WasapiProcessBackend::new(pid, ProcessMode::Include);
            let terminated = backend.termination_flag();
            (Box::new(backend), terminated, CaptureMode::ProcessLoopback)
        }
        CaptureSource::Process { .. } => {
            let backend = WasapiSystemBackend::new(false, None);
            let terminated = backend.termination_flag();
            (Box::new(backend), terminated, CaptureMode::OutputLoopback)
        }
        CaptureSource::OutputDevice { name } => {
            let backend = WasapiSystemBackend::new(false, name);
            let terminated = backend.termination_flag();
            (Box::new(backend), terminated, CaptureMode::OutputLoopback)
        }
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
    let (runtime_error_sender, runtime_error_receiver) = mpsc::sync_channel(1);
    let stop = Arc::new(AtomicBool::new(false));
    let drain_stop = Arc::clone(&stop);
    let drain = thread::Builder::new()
        .name("vapor-tap-pcm-drain".into())
        .spawn(move || {
            let packet_samples = (sample_rate as usize / 50).max(1) * usize::from(channels);
            let mut samples = vec![0.0; packet_samples];
            while !drain_stop.load(Ordering::Acquire) {
                if consumer.available() < packet_samples {
                    if backend_terminated.load(Ordering::Acquire) {
                        if !drain_stop.load(Ordering::Acquire) {
                            let _ = runtime_error_sender.try_send(Error::Native(
                                "Windows audio capture backend stopped unexpectedly".into(),
                            ));
                        }
                        break;
                    }
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                let count = consumer.pop_slice(&mut samples);
                if count == packet_samples {
                    let frame = AudioFrame {
                        format,
                        samples: samples.clone(),
                    };
                    match sender.try_send(frame) {
                        Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                        Err(mpsc::TrySendError::Disconnected(_)) => break,
                    }
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
        runtime_error_receiver,
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

pub(crate) fn process_isolation_supported() -> Result<bool> {
    Ok(windows_build()? >= 20_348)
}

pub(crate) fn list_output_devices() -> Result<Vec<OutputDevice>> {
    windows_backend::list_output_devices()
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

pub(crate) fn list_audio_applications() -> Result<Vec<AudioApplication>> {
    thread::Builder::new()
        .name("vapor-tap-audio-apps".into())
        .spawn(enumerate_audio_applications)
        .map_err(Error::Io)?
        .join()
        .map_err(|_| Error::Native("audio application enumeration thread panicked".into()))?
}

fn enumerate_audio_applications() -> Result<Vec<AudioApplication>> {
    let _com = ComGuard::new()?;
    let mut applications = BTreeMap::<u32, AudioApplication>::new();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|error| Error::Native(error.to_string()))?;
        let devices = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|error| Error::Native(error.to_string()))?;
        let device_count = devices
            .GetCount()
            .map_err(|error| Error::Native(error.to_string()))?;

        for device_index in 0..device_count {
            let Ok(device) = devices.Item(device_index) else {
                continue;
            };
            let device_name = endpoint_friendly_name(&device)
                .unwrap_or_else(|| format!("Output {}", device_index + 1));
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(sessions) = manager.GetSessionEnumerator() else {
                continue;
            };
            let Ok(session_count) = sessions.GetCount() else {
                continue;
            };
            for session_index in 0..session_count {
                let Ok(control) = sessions.GetSession(session_index) else {
                    continue;
                };
                if control.GetState().ok() != Some(AudioSessionStateActive) {
                    continue;
                }
                let Ok(control2): windows::core::Result<IAudioSessionControl2> = control.cast()
                else {
                    continue;
                };
                let Ok(pid) = control2.GetProcessId() else {
                    continue;
                };
                if pid == 0 || pid == std::process::id() {
                    continue;
                }
                let (process_name, identifier) = process_identity(pid)
                    .or_else(|| session_display_name(&control).map(|name| (name, None)))
                    .unwrap_or_else(|| (format!("PID {pid}"), None));
                let application = applications.entry(pid).or_insert_with(|| AudioApplication {
                    pid,
                    name: process_name,
                    identifier,
                    output_devices: Vec::new(),
                });
                if !application.output_devices.contains(&device_name) {
                    application.output_devices.push(device_name.clone());
                }
            }
        }
    }
    let mut applications: Vec<_> = applications.into_values().collect();
    applications.sort_by_key(|application| (application.name.to_lowercase(), application.pid));
    Ok(applications)
}

struct ComGuard;

impl ComGuard {
    fn new() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }
            .map_err(|error| Error::Native(error.to_string()))?;
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

unsafe fn endpoint_friendly_name(device: &IMMDevice) -> Option<String> {
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }.ok()?;
    let value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.ok()?;
    let name = value.to_string();
    (!name.is_empty()).then_some(name)
}

unsafe fn session_display_name(control: &IAudioSessionControl) -> Option<String> {
    let display = unsafe { control.GetDisplayName() }.ok()?;
    if display.is_null() {
        return None;
    }
    let name = unsafe { display.to_string() }
        .ok()
        .filter(|name| !name.is_empty());
    unsafe { CoTaskMemFree(Some(display.0.cast())) };
    name
}

unsafe fn process_identity(pid: u32) -> Option<(String, Option<String>)> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = vec![0_u16; 32_768];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    result.ok()?;
    let path = String::from_utf16(&buffer[..length as usize]).ok()?;
    let name = Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&path)
        .to_owned();
    Some((name, Some(path)))
}

impl PlatformSession {
    pub(crate) fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.stop.store(true, Ordering::Release);
        self.backend.stop();
        if let Some(drain) = self.drain.take() {
            drain
                .join()
                .map_err(|_| Error::Native("PCM drain thread panicked".into()))?;
        }
        self.stopped = true;
        Ok(())
    }
}
