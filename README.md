# vapor-tap

Cross-platform application audio capture for:

- Windows 10 (default output mix through WASAPI loopback)
- Windows 11 (WASAPI process loopback)
- macOS 14.2 or newer (Core Audio process taps)

The public Rust API is platform-neutral and produces interleaved `f32` PCM.
No virtual audio device or kernel driver is required.

## Build

```shell
cargo build --release
```

On Windows 10, a PID request automatically falls back to the complete default
output mix because PID-isolated loopback requires build 20348 or newer. This is
appropriate when the target application is the only active audio source.
macOS versions older than 14.2 remain unsupported.

## CLI smoke test

Start audio playback in the target application. On Windows 11 and macOS, Vapor
Tap discovers active audio applications and shows friendly names, so users do
not need to find a PID:

```shell
vapor-tap apps
vapor-tap capture --app WeChat --seconds 10 --output capture.wav
vapor-tap transcribe --app Chrome --funasr-url ws://127.0.0.1:10095
```

When all source options are omitted, Windows 11 and macOS display an interactive
numbered application picker. Windows 10 skips the picker and immediately
captures the complete default output mix because application selection cannot
provide isolation there:

```shell
vapor-tap transcribe --funasr-url ws://127.0.0.1:10095
```

`--pid` remains available for automation and advanced integrations:

```shell
vapor-tap capture --pid 1234 --seconds 10 --output capture.wav
```

The WAV file contains interleaved 32-bit IEEE-float samples in the native
capture format. On Windows 11, the target PID and its child-process tree are
included. On Windows 10, the PID is ignored and a warning reports that the
default output mix is being captured. On macOS, pass the PID of the process that
owns the Core Audio render stream. Multi-process applications such as WeChat
may move audio to a helper process on platforms using true process capture.

Windows output endpoints can also be selected explicitly:

```shell
vapor-tap devices
vapor-tap capture --default-device --seconds 10 --output capture.wav
vapor-tap capture --device "Speakers (Realtek Audio)" --seconds 10 --output capture.wav
```

## Remote FunASR transcription

FunASR runs independently and may be on another machine. Vapor Tap connects as
a WebSocket client; the capture machine does not need Python, Docker, models,
or a GPU.

```shell
vapor-tap transcribe \
  --app WeChat \
  --seconds 60 \
  --funasr-url wss://asr.example.com/ws \
  --mode two-pass \
  --text-output transcript.txt \
  --json-output transcript.jsonl
```

For the standard FunASR runtime whose WebSocket is directly exposed on port
10095, a local development command typically looks like:

```shell
vapor-tap transcribe --app WeChat --funasr-url ws://127.0.0.1:10095
```

Captured audio is downmixed and resampled in a worker thread to mono, 16 kHz,
signed PCM16 little-endian. It is sent as 60 ms binary messages. The audio does
not need to be stored locally. Add `--save-audio original.wav` to retain the
native float PCM at the same time.

`two-pass` is the default recognition mode. Online messages are emitted as
partial text and `2pass-offline` messages are appended as final text. The JSONL
output distinguishes `partial`, `final`, `server_error`, and `disconnected`
events. A bounded queue prevents unbounded memory growth; if it fills, the
command fails with `AudioQueueFull` rather than silently losing speech.

For bearer authentication, put the token in an environment variable rather
than the command line:

```shell
# PowerShell
$env:VAPOR_TAP_FUNASR_TOKEN = "secret"
vapor-tap transcribe --app WeChat --funasr-url wss://asr.example.com/ws
```

Use `wss://` over untrusted networks. A disconnect is reported explicitly and
the command exits; restarting creates a new ASR session because the model cache
from an interrupted WebSocket cannot be resumed safely.

## Library API

```rust,no_run
use vapor_tap::{CaptureConfig, CaptureSession, Result};

fn capture(pid: u32) -> Result<()> {
    let mut session = CaptureSession::start(CaptureConfig::for_pid(pid))?;
    while let Ok(frame) = session.frames().recv() {
        println!(
            "{} frames, {} Hz, {} channels",
            frame.frame_count(),
            frame.format.sample_rate,
            frame.format.channels
        );
    }
    session.stop()
}
```

The channel is bounded. Native realtime capture never waits for a slow
consumer; packets are dropped when the channel or intermediate ring is full.
Do not perform encoding or network I/O in a native audio callback.

## Windows details

On build 20348 or newer, the Windows backend activates
`VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK` with
`PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE`. It uses 48 kHz stereo
floating-point PCM and includes audio rendered by the PID's descendants.

On Windows 10 22H2 build 19045, `CaptureConfig::for_pid` transparently selects
classic `AUDCLNT_STREAMFLAGS_LOOPBACK` for the default render endpoint. It
captures all applications using that endpoint, including system sounds. No
virtual audio device or kernel driver is required for this fallback.

Application discovery enumerates active render endpoints and their active
`IAudioSessionControl2` sessions, obtains each session PID, and resolves the
process executable name. On Windows 10 this list is diagnostic only; the normal
no-source workflow intentionally skips selection and captures the default mix.
On Windows 11 the selected PID is passed to process loopback.

## macOS permissions and packaging

The distributed executable should be inside a signed application bundle with:

```xml
<key>LSMinimumSystemVersion</key>
<string>14.2</string>
<key>NSAudioCaptureUsageDescription</key>
<string>Capture audio from the application selected by the user.</string>
```

The user must grant the app access under **System Settings → Privacy &
Security → Screen & System Audio Recording**. A denied Core Audio tap is mapped
to `PermissionDenied` when Core Audio returns its usual permission error.

The macOS backend creates a private process tap and private aggregate device.
Both are stopped and destroyed when `CaptureSession` is stopped or dropped.
Application discovery reads Core Audio's process object list and keeps objects
whose `IsRunningOutput` property is true, exposing their PID and bundle ID.

## Current validation status

- Windows target: compiled and unit-tested on build 19045. Both explicit
  default-device capture and automatic PID fallback were exercised against a
  real Realtek output endpoint and produced non-silent PCM. Active session
  discovery resolved a live `pwsh.exe` audio stream and `capture --app pwsh`
  produced non-silent PCM. The no-source Win10 path skipped selection and also
  captured non-silent PCM. A Windows 11 PID smoke test is still required.
- macOS target: cross-compiled with `cargo check --target
  aarch64-apple-darwin`. Permission prompting and non-zero PCM require a macOS
  14.2+ machine for final validation.
