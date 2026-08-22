// Core Audio process-tap setup follows Apple's documented tap -> private
// aggregate-device -> IOProc chain. Parts of the low-level binding approach
// were informed by flexaudio-os-macos (MIT, Studio Sadola).
#![allow(unsafe_op_in_unsafe_fn)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::{MaybeUninit, size_of};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use block2::RcBlock;
use flexaudio_core::backend::RawSink;
use flexaudio_core::raw_ring;
use objc2::AnyThread;
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::ffi::NSInteger;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_core_audio::{
    AudioDeviceCreateIOProcIDWithBlock, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID,
    AudioDeviceStart, AudioDeviceStop, AudioHardwareCreateAggregateDevice,
    AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice,
    AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize,
    AudioObjectID, AudioObjectPropertyAddress, CATapDescription, kAudioAggregateDeviceIsPrivateKey,
    kAudioAggregateDeviceIsStackedKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey,
    kAudioAggregateDeviceUIDKey, kAudioHardwarePropertyProcessObjectList,
    kAudioHardwarePropertyTranslatePIDToProcessObject, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioProcessPropertyBundleID,
    kAudioProcessPropertyIsRunningOutput, kAudioProcessPropertyPID,
    kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{
    AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagIsFloat,
};
use objc2_core_foundation::{CFDictionary, CFString};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSObject, NSString, NSUUID};

use crate::{
    AudioApplication, AudioFormat, AudioFrame, CaptureConfig, CaptureMode, CaptureSource, Error,
    OutputDevice, Result,
};

const NO_ERR: i32 = 0;
#[allow(clippy::type_complexity)]
struct TapChain {
    aggregate_id: AudioObjectID,
    io_proc_id: AudioDeviceIOProcID,
    tap_id: AudioObjectID,
    stopped: Arc<AtomicBool>,
    _block: RcBlock<
        dyn Fn(
            NonNull<AudioTimeStamp>,
            NonNull<AudioBufferList>,
            NonNull<AudioTimeStamp>,
            NonNull<AudioBufferList>,
            NonNull<AudioTimeStamp>,
        ),
    >,
    _description: Retained<CATapDescription>,
}

impl Drop for TapChain {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        unsafe {
            if self.io_proc_id.is_some() {
                let _ = AudioDeviceStop(self.aggregate_id, self.io_proc_id);
                let _ = AudioDeviceDestroyIOProcID(self.aggregate_id, self.io_proc_id);
            }
            if self.aggregate_id != 0 {
                let _ = AudioHardwareDestroyAggregateDevice(self.aggregate_id);
            }
            if self.tap_id != 0 {
                let _ = AudioHardwareDestroyProcessTap(self.tap_id);
            }
        }
    }
}

pub(crate) struct PlatformSession {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    drain: Option<JoinHandle<()>>,
    stopped: bool,
}

pub(crate) fn start(
    config: CaptureConfig,
) -> Result<(PlatformSession, mpsc::Receiver<AudioFrame>, CaptureMode)> {
    ensure_supported_version()?;
    let pid = match config.source {
        CaptureSource::Process { pid } => pid,
        CaptureSource::OutputDevice { .. } => {
            return Err(Error::UnsupportedPlatform(
                "output endpoint capture is available only on Windows",
            ));
        }
    };
    let (producer, mut consumer) = raw_ring(48_000 * 2 * 2);
    let sink = RawSink::new(producer, 48_000, 2);
    let (sender, receiver) = mpsc::sync_channel(config.channel_capacity);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let (ready_sender, ready_receiver) = mpsc::sync_channel(1);

    let handle = thread::Builder::new()
        .name("vapor-tap-coreaudio".into())
        .spawn(move || {
            let result = (|| unsafe {
                let process_object = translate_pid(pid)?;
                let (chain, format) = build_tap_chain(process_object, sink)?;
                let _ = ready_sender.send(Ok(format));
                while !thread_stop.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(10));
                }
                drop(chain);
                Ok::<_, Error>(())
            })();
            if let Err(error) = result {
                let _ = ready_sender.send(Err(error));
            }
        })
        .map_err(Error::Io)?;

    match ready_receiver.recv() {
        Ok(Ok(format)) => {
            let drain_stop = Arc::clone(&stop);
            let drain = match thread::Builder::new()
                .name("vapor-tap-pcm-drain".into())
                .spawn(move || {
                    let packet_samples =
                        (format.sample_rate as usize / 50) * usize::from(format.channels);
                    let mut samples = vec![0.0; packet_samples];
                    while !drain_stop.load(Ordering::Acquire) {
                        if consumer.available() < packet_samples {
                            thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        if consumer.pop_slice(&mut samples) == packet_samples {
                            let _ = sender.try_send(AudioFrame {
                                format,
                                samples: samples.clone(),
                            });
                        }
                    }
                }) {
                Ok(drain) => drain,
                Err(error) => {
                    stop.store(true, Ordering::Release);
                    let _ = handle.join();
                    return Err(Error::Io(error));
                }
            };
            Ok((
                PlatformSession {
                    stop,
                    thread: Some(handle),
                    drain: Some(drain),
                    stopped: false,
                },
                receiver,
                CaptureMode::ProcessLoopback,
            ))
        }
        Ok(Err(error)) => {
            let _ = handle.join();
            Err(error)
        }
        Err(_) => {
            let _ = handle.join();
            Err(Error::Native(
                "Core Audio capture thread exited during startup".into(),
            ))
        }
    }
}

pub(crate) fn list_output_devices() -> Result<Vec<OutputDevice>> {
    Err(Error::UnsupportedPlatform(
        "output endpoint enumeration is available only on Windows",
    ))
}

pub(crate) fn list_audio_applications() -> Result<Vec<AudioApplication>> {
    ensure_supported_version()?;
    let objects = process_object_list()?;
    let mut applications = Vec::new();
    for object in objects {
        let is_running = unsafe {
            audio_property::<u32>(object, kAudioProcessPropertyIsRunningOutput).unwrap_or(0)
        };
        if is_running == 0 {
            continue;
        }
        let Some(pid) = (unsafe { audio_property::<i32>(object, kAudioProcessPropertyPID) }) else {
            continue;
        };
        let Ok(pid) = u32::try_from(pid) else {
            continue;
        };
        if pid == 0 || pid == std::process::id() {
            continue;
        }
        let identifier = unsafe { process_bundle_identifier(object) };
        let name = identifier
            .as_deref()
            .and_then(|identifier| identifier.rsplit('.').next())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("PID {pid}"));
        applications.push(AudioApplication {
            pid,
            name,
            identifier,
            output_devices: Vec::new(),
        });
    }
    applications.sort_by_key(|application| (application.name.to_lowercase(), application.pid));
    applications.dedup_by_key(|application| application.pid);
    Ok(applications)
}

pub(crate) fn process_isolation_supported() -> Result<bool> {
    ensure_supported_version()?;
    Ok(true)
}

fn process_object_list() -> Result<Vec<AudioObjectID>> {
    let mut address = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyProcessObjectList,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut byte_count = 0_u32;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&mut address),
            0,
            std::ptr::null(),
            NonNull::from(&mut byte_count),
        )
    };
    check_status("read audio process list size", status)?;
    let count = byte_count as usize / size_of::<AudioObjectID>();
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut objects = vec![0; count];
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as AudioObjectID,
            NonNull::from(&mut address),
            0,
            std::ptr::null(),
            NonNull::from(&mut byte_count),
            NonNull::new(objects.as_mut_ptr().cast()).expect("non-empty object list"),
        )
    };
    check_status("read audio process list", status)?;
    objects.truncate(byte_count as usize / size_of::<AudioObjectID>());
    Ok(objects)
}

unsafe fn audio_property<T: Copy>(object: AudioObjectID, selector: u32) -> Option<T> {
    let mut address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut byte_count = size_of::<T>() as u32;
    let mut value = MaybeUninit::<T>::uninit();
    let status = AudioObjectGetPropertyData(
        object,
        NonNull::from(&mut address),
        0,
        std::ptr::null(),
        NonNull::from(&mut byte_count),
        NonNull::new(value.as_mut_ptr().cast())?,
    );
    (status == NO_ERR && byte_count as usize == size_of::<T>()).then(|| value.assume_init())
}

unsafe fn process_bundle_identifier(object: AudioObjectID) -> Option<String> {
    let bundle = audio_property::<*const CFString>(object, kAudioProcessPropertyBundleID)?;
    bundle.as_ref().map(ToString::to_string)
}

impl PlatformSession {
    pub(crate) fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.thread.take() {
            handle
                .join()
                .map_err(|_| Error::Native("Core Audio capture thread panicked".into()))?;
        }
        if let Some(handle) = self.drain.take() {
            handle
                .join()
                .map_err(|_| Error::Native("PCM drain thread panicked".into()))?;
        }
        self.stopped = true;
        Ok(())
    }
}

unsafe fn translate_pid(pid: u32) -> Result<AudioObjectID> {
    let address = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyTranslatePIDToProcessObject,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let pid = pid as i32;
    let mut object_id: AudioObjectID = 0;
    let mut size = size_of::<AudioObjectID>() as u32;
    let status = AudioObjectGetPropertyData(
        kAudioObjectSystemObject as AudioObjectID,
        NonNull::from(&address),
        size_of::<i32>() as u32,
        (&pid as *const i32).cast::<c_void>(),
        NonNull::from(&mut size),
        NonNull::new_unchecked((&mut object_id as *mut AudioObjectID).cast::<c_void>()),
    );
    check_status("translate PID to audio process", status)?;
    if object_id == 0 {
        return Err(Error::ProcessNotFound(pid as u32));
    }
    Ok(object_id)
}

unsafe fn build_tap_chain(
    process_object: AudioObjectID,
    sink: RawSink,
) -> Result<(TapChain, AudioFormat)> {
    let process_number = NSNumber::numberWithUnsignedInt(process_object);
    let processes = NSArray::from_retained_slice(&[process_number]);
    let description =
        CATapDescription::initStereoMixdownOfProcesses(CATapDescription::alloc(), &processes);
    description.setName(&NSString::from_str("Vapor Tap Process Capture"));
    description.setPrivate(true);
    let tap_uid = description.UUID().UUIDString();

    let mut tap_id = 0;
    check_status(
        "create process tap",
        AudioHardwareCreateProcessTap(Some(&description), &mut tap_id),
    )?;
    if tap_id == 0 {
        return Err(Error::Native("Core Audio returned an empty tap ID".into()));
    }
    let format = match read_tap_format(tap_id) {
        Ok(format) => format,
        Err(error) => {
            let _ = AudioHardwareDestroyProcessTap(tap_id);
            return Err(error);
        }
    };

    let aggregate_id = match create_aggregate_device(&tap_uid) {
        Ok(id) => id,
        Err(error) => {
            let _ = AudioHardwareDestroyProcessTap(tap_id);
            return Err(error);
        }
    };

    let callback_stopped = Arc::new(AtomicBool::new(false));
    let stopped_for_block = Arc::clone(&callback_stopped);
    let sink = RefCell::new(sink);
    let scratch_capacity = format.sample_rate as usize * usize::from(format.channels) / 2;
    let scratch = RefCell::new(Vec::<f32>::with_capacity(scratch_capacity));
    let block = RcBlock::new(
        move |_now: NonNull<AudioTimeStamp>,
              input: NonNull<AudioBufferList>,
              _input_time: NonNull<AudioTimeStamp>,
              _output: NonNull<AudioBufferList>,
              _output_time: NonNull<AudioTimeStamp>| {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                if stopped_for_block.load(Ordering::Acquire) {
                    return;
                }
                if let (Ok(mut sink), Ok(mut scratch)) =
                    (sink.try_borrow_mut(), scratch.try_borrow_mut())
                {
                    let samples = unsafe { copy_buffer_list(input.as_ptr(), &mut scratch) };
                    if !samples.is_empty() {
                        sink.push(samples, 0);
                    }
                }
            }));
        },
    );

    let mut io_proc_id: AudioDeviceIOProcID = None;
    let status = AudioDeviceCreateIOProcIDWithBlock(
        NonNull::from(&mut io_proc_id),
        aggregate_id,
        None,
        RcBlock::as_ptr(&block),
    );
    if status != NO_ERR || io_proc_id.is_none() {
        let _ = AudioHardwareDestroyAggregateDevice(aggregate_id);
        let _ = AudioHardwareDestroyProcessTap(tap_id);
        return Err(native_error("create Core Audio IOProc", status));
    }

    let status = AudioDeviceStart(aggregate_id, io_proc_id);
    if status != NO_ERR {
        let _ = AudioDeviceDestroyIOProcID(aggregate_id, io_proc_id);
        let _ = AudioHardwareDestroyAggregateDevice(aggregate_id);
        let _ = AudioHardwareDestroyProcessTap(tap_id);
        return Err(native_error("start Core Audio aggregate device", status));
    }

    Ok((
        TapChain {
            aggregate_id,
            io_proc_id,
            tap_id,
            stopped: callback_stopped,
            _block: block,
            _description: description,
        },
        format,
    ))
}

unsafe fn read_tap_format(tap_id: AudioObjectID) -> Result<AudioFormat> {
    let address = AudioObjectPropertyAddress {
        mSelector: kAudioTapPropertyFormat,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut description: AudioStreamBasicDescription = std::mem::zeroed();
    let mut size = size_of::<AudioStreamBasicDescription>() as u32;
    check_status(
        "read process tap format",
        AudioObjectGetPropertyData(
            tap_id,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(
                (&mut description as *mut AudioStreamBasicDescription).cast::<c_void>(),
            ),
        ),
    )?;
    if description.mFormatFlags & kAudioFormatFlagIsFloat == 0 {
        return Err(Error::Native(
            "Core Audio process tap did not provide floating-point PCM".into(),
        ));
    }
    let sample_rate = description.mSampleRate.round() as u32;
    let channels = description.mChannelsPerFrame as u16;
    if sample_rate == 0 || channels == 0 {
        return Err(Error::Native(
            "Core Audio returned an invalid tap format".into(),
        ));
    }
    Ok(AudioFormat {
        sample_rate,
        channels,
    })
}

unsafe fn create_aggregate_device(tap_uid: &NSString) -> Result<AudioObjectID> {
    let yes = NSNumber::numberWithBool(true);
    let no = NSNumber::numberWithBool(false);
    let sub_tap: Retained<NSDictionary<NSString, NSObject>> = NSDictionary::from_slices::<NSString>(
        &[
            &key(kAudioSubTapUIDKey),
            &key(kAudioSubTapDriftCompensationKey),
        ],
        &[tap_uid.as_ref(), yes.as_ref()],
    );
    let tap_list: Retained<NSArray<NSObject>> =
        NSArray::from_retained_slice(&[Retained::into_super(sub_tap)]);
    let name = NSString::from_str("Vapor Tap Private Aggregate");
    let uid = NSUUID::new().UUIDString();
    let keys: [&NSString; 6] = [
        &key(kAudioAggregateDeviceNameKey),
        &key(kAudioAggregateDeviceUIDKey),
        &key(kAudioAggregateDeviceIsPrivateKey),
        &key(kAudioAggregateDeviceIsStackedKey),
        &key(kAudioAggregateDeviceTapAutoStartKey),
        &key(kAudioAggregateDeviceTapListKey),
    ];
    let values: [&NSObject; 6] = [
        name.as_ref(),
        uid.as_ref(),
        yes.as_ref(),
        no.as_ref(),
        yes.as_ref(),
        tap_list.as_ref(),
    ];
    let dictionary: Retained<NSDictionary<NSString, NSObject>> =
        NSDictionary::from_slices::<NSString>(&keys, &values);
    let cf_dictionary = &*(Retained::as_ptr(&dictionary) as *const CFDictionary);
    let mut aggregate_id = 0;
    check_status(
        "create private aggregate device",
        AudioHardwareCreateAggregateDevice(cf_dictionary, NonNull::from(&mut aggregate_id)),
    )?;
    if aggregate_id == 0 {
        return Err(Error::Native(
            "Core Audio returned an empty aggregate device ID".into(),
        ));
    }
    Ok(aggregate_id)
}

fn key(value: &std::ffi::CStr) -> Retained<NSString> {
    NSString::from_str(value.to_str().unwrap_or(""))
}

unsafe fn copy_buffer_list(list: *const AudioBufferList, scratch: &mut Vec<f32>) -> &[f32] {
    if list.is_null() || (*list).mNumberBuffers == 0 {
        return &[];
    }
    let buffers =
        std::slice::from_raw_parts((*list).mBuffers.as_ptr(), (*list).mNumberBuffers as usize);
    if buffers.len() == 1 {
        let buffer = &buffers[0];
        if buffer.mData.is_null() {
            return &[];
        }
        let count = buffer.mDataByteSize as usize / size_of::<f32>();
        scratch.clear();
        scratch.extend_from_slice(std::slice::from_raw_parts(
            buffer.mData.cast::<f32>(),
            count,
        ));
        return scratch;
    }

    let frame_count = buffers
        .iter()
        .map(|buffer| buffer.mDataByteSize as usize / size_of::<f32>())
        .min()
        .unwrap_or(0);
    if frame_count == 0 || buffers.iter().any(|buffer| buffer.mData.is_null()) {
        return &[];
    }
    scratch.resize(frame_count * buffers.len(), 0.0);
    for (channel, buffer) in buffers.iter().enumerate() {
        let source = std::slice::from_raw_parts(buffer.mData.cast::<f32>(), frame_count);
        for (frame, sample) in source.iter().enumerate() {
            scratch[frame * buffers.len() + channel] = *sample;
        }
    }
    scratch
}

fn check_status(context: &str, status: i32) -> Result<()> {
    if status == NO_ERR {
        Ok(())
    } else {
        Err(native_error(context, status))
    }
}

fn native_error(context: &str, status: i32) -> Error {
    if status == i32::from_be_bytes(*b"nope") {
        return Error::PermissionDenied;
    }
    let bytes = (status as u32).to_be_bytes();
    let detail = if bytes.iter().all(u8::is_ascii_graphic) {
        format!("'{}'", String::from_utf8_lossy(&bytes))
    } else {
        status.to_string()
    };
    Error::Native(format!("{context}: OSStatus {detail}"))
}

fn ensure_supported_version() -> Result<()> {
    let (major, minor) = current_os_version();
    if major > 14 || (major == 14 && minor >= 2) {
        Ok(())
    } else {
        Err(Error::UnsupportedOsVersion)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct OsVersion {
    major: NSInteger,
    minor: NSInteger,
    patch: NSInteger,
}

unsafe impl Encode for OsVersion {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[
            NSInteger::ENCODING,
            NSInteger::ENCODING,
            NSInteger::ENCODING,
        ],
    );
}

unsafe impl RefEncode for OsVersion {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

fn current_os_version() -> (i64, i64) {
    unsafe {
        let process_info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
        let version: OsVersion = msg_send![process_info, operatingSystemVersion];
        (version.major as i64, version.minor as i64)
    }
}
