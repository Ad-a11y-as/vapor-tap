# vapor-tap

English | [简体中文](README.zh-CN.md)

Cross-platform application audio capture for:

- Windows 10 (default output mix through WASAPI loopback)
- Windows 11 (system mix by default, optional WASAPI process loopback)
- macOS 14.2 or newer (global system-audio or per-process Core Audio taps)

The public Rust API is platform-neutral and produces interleaved `f32` PCM.
No virtual audio device or kernel driver is required.
FunASR is an optional transcription backend; application discovery, capture,
and WAV recording work without installing or connecting to FunASR.

## Purpose

Vapor Tap provides a lightweight, programmable audio path between desktop
applications and downstream audio services. It can:

- Capture audio played by messaging apps, browsers, media players, and meeting
  software for recording, archiving, or analysis.
- Stream audio to a local or remote FunASR service for live transcription of
  meetings, courses, videos, and calls.
- Produce native WAV recordings, plain text, and JSONL events for subtitle
  generation, meeting notes, search, and other automated workflows.
- Run as a Rust library so another application can consume uniform PCM audio
  frames without managing a CLI subprocess.
- Discover active audio applications and select them by name when isolation is
  needed, so users do not have to locate process IDs manually.

## Key advantages

- **One cross-platform interface:** Windows 10, Windows 11, and macOS share the
  same CLI and Rust API while platform-specific behavior stays inside the
  capture backend.
- **No virtual audio driver:** Direct WASAPI and Core Audio integration avoids
  installing a virtual sound card or kernel driver.
- **Useful by default:** With no source option, Windows and macOS capture system
  audio immediately. Use `--app` or `--pid` only when isolation is needed.
- **Multi-process application support:** Windows 11 includes the selected
  process tree, which helps with browsers and messaging apps that render audio
  from helper processes.
- **Capture and ASR are decoupled:** FunASR is optional and can run on another
  machine. WAV recording and application discovery continue to work without it.
- **Designed for realtime pipelines:** Format conversion and network work stay
  outside native realtime callbacks, and bounded queues prevent uncontrolled
  memory growth.
- **Explicit failure reporting:** Backend termination, network disconnects,
  server errors, and ASR queue pressure are surfaced as errors instead of being
  mistaken for silence or successful completion.
- **Integration-ready output:** Lossless float PCM WAV, plain text, and
  structured JSONL support recording, encoding, storage, and downstream
  application processing.

## Build

```shell
cargo build --release
```

On Windows 10, a PID request automatically falls back to the complete default
output mix because PID-isolated loopback requires build 20348 or newer. This is
appropriate when the target application is the only active audio source.
macOS versions older than 14.2 remain unsupported.

## CLI smoke test

Start audio playback before capture. With no source option, every supported
platform captures system-wide output audio without showing an application
picker:

```shell
vapor-tap capture
vapor-tap transcribe --funasr-url ws://127.0.0.1:10095
```

When `--seconds` is omitted, recording continues until you press `Ctrl+C`.
Vapor Tap then stops capture, flushes the samples, finalizes the WAV header, and
exits normally. Add a value such as `--seconds 60` for a fixed-duration run; a
timed recording can also be ended early with `Ctrl+C` without corrupting the
WAV file.

When `--output` is omitted, the file uses the local-time name
`captured-YYYYMMDD-HHMMSS.wav` in the command's current working directory. The
complete path is printed when capture finishes. You can still provide a name or
path such as `--output recordings/meeting.wav`.

To isolate an application on Windows 11 or macOS, list active applications and
select one by name; users do not need to find a PID:

```shell
vapor-tap apps
vapor-tap capture --app WeChat --seconds 10 --output capture.wav
vapor-tap transcribe --app Chrome --funasr-url ws://127.0.0.1:10095
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

## Optional remote FunASR transcription

FunASR is required only for live speech-to-text. Audio capture and WAV recording
do not depend on it. FunASR runs independently and may be on another machine.
Vapor Tap connects as a WebSocket client; the capture machine does not need
Python, Docker, models, or a GPU.

```shell
vapor-tap transcribe \
  --app WeChat \
  --funasr-url wss://asr.example.com/ws \
  --mode two-pass \
  --json-output transcript.jsonl
```

When `--seconds` is omitted, transcription runs continuously. Pressing
`Ctrl+C` once stops capture, sends the end-of-input message, waits for the final
FunASR result, and then flushes and closes the text, JSONL, and optional WAV
files. Use a value such as `--seconds 60` for unattended fixed-duration runs.
`Ctrl+C` can also end a timed run early while preserving the same graceful
shutdown sequence.

When `--text-output` is omitted, final recognition text is saved in the current
working directory as `transcript-YYYYMMDD-HHMMSS.txt` using local time. The
complete path is printed on completion. Use a value such as
`--text-output records/meeting.txt` to choose a different name or location.

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
The certificate must be trusted by the client operating system and match the
server address. Vapor Tap does not bypass TLS verification. For a self-signed
certificate, install its issuing CA in the client trust store or terminate TLS
at a trusted reverse proxy.

## FunASR hardware requirements

The `funasr-runtime-sdk-online-cpu` service used by this project does not
require a GPU. FunASR's
[official realtime deployment guide](https://github.com/modelscope/FunASR/blob/main/runtime/docs/SDK_tutorial_online_zh.md)
lists the following recommended x86 compute-server configurations:

| CPU | Memory | Approximate concurrent sessions |
| --- | ---: | ---: |
| 4 vCPU cores | 8 GB | 16 |
| 16 vCPU cores | 32 GB | 32 |
| 64 vCPU cores | 128 GB | 100 |

These are recommended sizing points and approximate capacity figures, not hard
minimum requirements. Actual capacity depends on CPU performance, model choice,
audio workload, and thread settings. For one Vapor Tap transcription stream, a
Linux host with 4 CPU cores and 8 GB of memory is a practical starting point;
8 cores and 16 GB or more are preferable for multiple streams or continuous
production use.

The current CPU image supports AMD64 and ARM64, although the published capacity
figures above are for x86 servers. The convenience deployment script supports
Linux; Windows and macOS can run Vapor Tap and connect to a FunASR service on a
Linux host or Linux container. The official documentation does not publish a
fixed disk-space minimum. As a planning allowance, reserve at least 15–20 GB
for the Docker image, the offline and streaming ASR models, VAD, punctuation,
language-model and ITN assets, and logs, then confirm the actual size of the
downloaded files.

The server derives decoder and I/O thread counts from the available CPU by
default. When tuning manually, FunASR recommends keeping
`decoder-thread-num × model-thread-num` close to the total CPU thread count.
GPU-based FunASR runtimes have separate model-specific CUDA and VRAM
requirements and are not required for this integration.

## Optional air-gapped FunASR installation

Here, *air-gapped installation* means deploying FunASR on a machine without
Internet access. It is different from FunASR's `offline` recognition mode.
Vapor Tap uses `two-pass` by default, so prepare FunASR's online/two-pass
WebSocket runtime rather than the runtime intended only for offline file
transcription.

The following procedure uses the official CPU Docker image. Run the commands
on Linux or another POSIX environment that supports Linux containers. Vapor
Tap itself may run on Windows or macOS and connect to this server over the
network. Docker Engine must already be installed on the offline machine, and
the connected staging machine should use the same CPU architecture. The image
version follows the
[official FunASR realtime service guide](https://github.com/modelscope/FunASR/blob/main/runtime/docs/SDK_advanced_guide_online.md).
Prepare both the image and models again when upgrading it.

### 1. Download the image and models on a connected machine

Pull the pinned image, create the host model directory, and open a shell in a
temporary staging container. This command opens Bash; it does not start the
FunASR service by itself:

```shell
FUNASR_IMAGE=registry.cn-hangzhou.aliyuncs.com/funasr_repo/funasr:funasr-runtime-sdk-online-cpu-0.1.13

mkdir -p funasr-airgap/models
docker pull "$FUNASR_IMAGE"
docker run --rm -it --name funasr-prefetch \
  --entrypoint bash -p 10095:10095 \
  -v "$PWD/funasr-airgap/models:/workspace/models" \
  "$FUNASR_IMAGE"
```

Inside the container, start the service with an explicit model list. Using the
same list here and on the offline machine prevents the image defaults from
silently selecting a different model set:

```shell
cd /workspace/FunASR/runtime
. ./run_server_2pass.sh \
  --download-model-dir /workspace/models \
  --model-dir damo/speech_paraformer-large-vad-punc_asr_nat-zh-cn-16k-common-vocab8404-onnx \
  --online-model-dir damo/speech_paraformer-large_asr_nat-zh-cn-16k-common-vocab8404-online-onnx \
  --vad-dir damo/speech_fsmn_vad_zh-cn-16k-common-onnx \
  --punc-dir damo/punc_ct-transformer_zh-cn-common-vad_realtime-vocab272727-onnx \
  --lm-dir damo/speech_ngram_lm_zh-cn-ai-wesp-fst \
  --itn-dir thuduj12/fst_itn_zh \
  --certfile 0
server_pid=$!
trap "kill -TERM $server_pid; wait $server_pid" TERM INT
wait "$server_pid"
```

The official script starts the server as a background process. Sourcing it
keeps that process attached to the current shell, while `wait` keeps the
container running and displays the server logs directly in the terminal.

### 2. Verify and package the offline bundle

Leave the staging container running. In another terminal on the connected
machine, verify that every required model directory exists:

```shell
for model in \
  damo/speech_paraformer-large-vad-punc_asr_nat-zh-cn-16k-common-vocab8404-onnx \
  damo/speech_paraformer-large_asr_nat-zh-cn-16k-common-vocab8404-online-onnx \
  damo/speech_fsmn_vad_zh-cn-16k-common-onnx \
  damo/punc_ct-transformer_zh-cn-common-vad_realtime-vocab272727-onnx \
  damo/speech_ngram_lm_zh-cn-ai-wesp-fst \
  thuduj12/fst_itn_zh
do
  test -d "funasr-airgap/models/$model" || {
    echo "missing model directory: $model"
    exit 1
  }
done
echo "all required model directories are present"
```

Wait until the service log reports no download or model-loading errors and port
`10095` is listening. Play some speech audio and run one end-to-end test from a
machine that can reach the staging server:

```shell
vapor-tap transcribe --funasr-url ws://127.0.0.1:10095 --seconds 10
```

Replace `127.0.0.1` with the staging server address when Vapor Tap runs on
another machine. After a successful test, stop the temporary container from the
second terminal:

```shell
docker stop funasr-prefetch
```

Because the model directory is mounted from the host, `--rm` removes only the
temporary container. Package the pinned image, verified models, and checksums:

```shell
docker save -o funasr-image.tar "$FUNASR_IMAGE"
tar -C funasr-airgap -czf funasr-models.tar.gz models
sha256sum funasr-image.tar funasr-models.tar.gz > SHA256SUMS
```

Copy `funasr-image.tar`, `funasr-models.tar.gz`, and `SHA256SUMS` to the
air-gapped machine.

### 3. Import and verify on the air-gapped machine

```shell
sha256sum -c SHA256SUMS
docker load -i funasr-image.tar
mkdir -p funasr-runtime-resources
tar -C funasr-runtime-resources -xzf funasr-models.tar.gz
```

The checksum protects the transferred archives. The model-directory check from
the previous step may also be repeated with
`funasr-runtime-resources/models/$model` as the base path.

### 4. Run an interactive smoke test

The following command creates a disposable container and opens Bash. It is
intended only for initial verification; exiting it removes the container:

```shell
FUNASR_IMAGE=registry.cn-hangzhou.aliyuncs.com/funasr_repo/funasr:funasr-runtime-sdk-online-cpu-0.1.13

docker run --rm -it --name funasr-smoke \
  --entrypoint bash -p 10095:10095 \
  -v "$PWD/funasr-runtime-resources/models:/workspace/models" \
  "$FUNASR_IMAGE"
```

Inside the container, use explicit local model paths. This prevents the runtime
from trying to resolve ModelScope model IDs:

```shell
cd /workspace/FunASR/runtime
. ./run_server_2pass.sh \
  --download-model-dir /workspace/models \
  --model-dir /workspace/models/damo/speech_paraformer-large-vad-punc_asr_nat-zh-cn-16k-common-vocab8404-onnx \
  --online-model-dir /workspace/models/damo/speech_paraformer-large_asr_nat-zh-cn-16k-common-vocab8404-online-onnx \
  --vad-dir /workspace/models/damo/speech_fsmn_vad_zh-cn-16k-common-onnx \
  --punc-dir /workspace/models/damo/punc_ct-transformer_zh-cn-common-vad_realtime-vocab272727-onnx \
  --lm-dir /workspace/models/damo/speech_ngram_lm_zh-cn-ai-wesp-fst \
  --itn-dir /workspace/models/thuduj12/fst_itn_zh \
  --certfile 0
server_pid=$!
trap "kill -TERM $server_pid; wait $server_pid" TERM INT
wait "$server_pid"
```

Run the Vapor Tap test from another terminal. Then press `Ctrl+C` in the
container terminal or run `docker stop funasr-smoke` from the second terminal.

### 5. Run FunASR as a persistent service

After the smoke test succeeds, create a detached container without `--rm`.
The restart policy starts it again after a host or Docker restart:

```shell
FUNASR_IMAGE=registry.cn-hangzhou.aliyuncs.com/funasr_repo/funasr:funasr-runtime-sdk-online-cpu-0.1.13

docker run -d --name funasr --restart unless-stopped \
  --entrypoint bash -p 10095:10095 \
  -v "$PWD/funasr-runtime-resources/models:/workspace/models" \
  "$FUNASR_IMAGE" -lc '
cd /workspace/FunASR/runtime
. ./run_server_2pass.sh \
  --download-model-dir /workspace/models \
  --model-dir /workspace/models/damo/speech_paraformer-large-vad-punc_asr_nat-zh-cn-16k-common-vocab8404-onnx \
  --online-model-dir /workspace/models/damo/speech_paraformer-large_asr_nat-zh-cn-16k-common-vocab8404-online-onnx \
  --vad-dir /workspace/models/damo/speech_fsmn_vad_zh-cn-16k-common-onnx \
  --punc-dir /workspace/models/damo/punc_ct-transformer_zh-cn-common-vad_realtime-vocab272727-onnx \
  --lm-dir /workspace/models/damo/speech_ngram_lm_zh-cn-ai-wesp-fst \
  --itn-dir /workspace/models/thuduj12/fst_itn_zh \
  --certfile 0
server_pid=$!
trap "kill -TERM $server_pid; wait $server_pid" TERM INT
wait "$server_pid"
'
```

Inspect and manage the persistent service with:

```shell
docker logs -f funasr
docker stop funasr
docker start funasr
```

For a local client, connect to `ws://127.0.0.1:10095`. If FunASR runs on
another LAN host, use that server's address and expose plain `ws://` only on a
trusted network. The Vapor Tap capture machine does not need FunASR, Python, or
the models installed locally. If any local model directory is missing, return
to the connected staging machine, complete the download and end-to-end test,
and rebuild the offline bundle.

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
process executable name. The normal no-source workflow captures the default
mix on every Windows version. On Windows 11 an explicitly selected application
PID is passed to process loopback; on Windows 10 it falls back to the mix.

## macOS permissions and packaging

The distributed executable should be inside a signed application bundle with:

```xml
<key>LSMinimumSystemVersion</key>
<string>14.2</string>
<key>NSAudioCaptureUsageDescription</key>
<string>Capture system or application audio selected by the user.</string>
```

The user must grant the app access under **System Settings → Privacy &
Security → Screen & System Audio Recording**. A denied Core Audio tap is mapped
to `PermissionDenied` when Core Audio returns its usual permission error.

The macOS backend creates a private global tap for the default no-source flow or
a private process tap for `--app`/`--pid`, together with a private aggregate
device. All are stopped and destroyed when `CaptureSession` is stopped or
dropped. Application discovery reads Core Audio's process object list and keeps
objects whose `IsRunningOutput` property is true, exposing their PID and bundle
ID.

## Current validation status

- Windows target: compiled and unit-tested on build 19045, and the real WASAPI
  loopback path was exercised. The latest saved WAV contained only zero
  samples, so non-silent capture remains under investigation and must not be
  treated as validated. A Windows 11 PID smoke test is also still required.
- macOS target: global and process taps cross-compile with `cargo check --target
  aarch64-apple-darwin`. Permission prompting and non-zero PCM require a macOS
  14.2+ machine for final validation.
- FunASR: the WebSocket upload, online/offline responses, and final
  acknowledgement were exercised against a real remote service through a
  temporary TLS proxy because its certificate was untrusted. The latest Windows
  input was all-zero PCM, so the server's text was a silence hallucination and
  does not validate recognition quality.
