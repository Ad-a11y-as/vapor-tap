# vapor-tap

Native per-process audio capture for:

- Windows 11 (WASAPI process loopback)
- macOS 14.2 or newer (Core Audio process taps)

The public Rust API is platform-neutral and produces interleaved `f32` PCM.
No virtual audio device or kernel driver is required.

## Build

```shell
cargo build --release
```

The crate is intentionally unsupported on Windows builds older than 20348 and
macOS versions older than 14.2. It returns `UnsupportedOsVersion` before trying
to create an audio stream.

## CLI smoke test

Find the PID that owns the application's audio output, then run:

```shell
vapor-tap capture --pid 1234 --seconds 10 --output capture.wav
```

The WAV file contains interleaved 32-bit IEEE-float samples in the native
capture format. On Windows, the target PID and its child-process tree are
included. On macOS, pass the PID of the process that owns the Core Audio render
stream. Multi-process applications such as WeChat may move audio to a helper
process, so production integration should track and restart capture when that
audio process changes.

## Remote FunASR transcription

FunASR runs independently and may be on another machine. Vapor Tap connects as
a WebSocket client; the capture machine does not need Python, Docker, models,
or a GPU.

```shell
vapor-tap transcribe \
  --pid 1234 \
  --seconds 60 \
  --funasr-url wss://asr.example.com/ws \
  --mode two-pass \
  --text-output transcript.txt \
  --json-output transcript.jsonl
```

For the standard FunASR runtime whose WebSocket is directly exposed on port
10095, a local development command typically looks like:

```shell
vapor-tap transcribe --pid 1234 --funasr-url ws://127.0.0.1:10095
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
vapor-tap transcribe --pid 1234 --funasr-url wss://asr.example.com/ws
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

The Windows backend activates `VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK` with
`PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE`. It uses 48 kHz stereo
floating-point PCM and includes audio rendered by the PID's descendants.

Windows 10 22H2 build 19045 is rejected because Microsoft requires build 20348
or later for process loopback.

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

## Current validation status

- Windows target: compiled and unit-tested on build 19045; the version rejection
  path was exercised. A Windows 11 audio-device smoke test is still required.
- macOS target: cross-compiled with `cargo check --target
  aarch64-apple-darwin`. Permission prompting and non-zero PCM require a macOS
  14.2+ machine for final validation.
