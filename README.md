# vapor-tap

## 中文说明

Vapor Tap 是一个使用 Rust 实现的跨平台应用音频捕获工具，可独立捕获声音并保存为 WAV。FunASR 只是可选的语音转文字后端：不安装、不配置或不连接 FunASR，也可以正常完成应用发现、音频捕获和本地录音。Vapor Tap 不需要安装虚拟声卡或内核驱动。

支持的平台和捕获方式：

- Windows 10 22H2（build 19045）：默认通过 WASAPI 回环捕获系统混合音频。该系统不支持按进程隔离；即使指定应用，也会退化为系统混音。
- Windows 11（build 20348 或更高版本）：默认通过 WASAPI 捕获系统混合音频；使用 `--app` 或 `--pid` 时，通过进程回环隔离目标应用及其子进程。
- macOS 14.2 或更高版本：默认通过 Core Audio 全局 Tap 捕获系统音频；使用 `--app` 或 `--pid` 时改用进程 Tap。

公共 Rust API 与平台无关，输出交错排列的 `f32` PCM 音频数据。

### 项目作用

Vapor Tap 在桌面应用与音频处理服务之间提供一条轻量、可编程的音频通道，适合以下场景：

- 捕获微信、浏览器、视频播放器、会议软件等应用正在播放的声音，用于录音、归档或后续分析。
- 将音频实时发送给本机或远程 FunASR，把会议、课程、视频和通话内容转换成文字。
- 同时输出 WAV 原始录音、纯文本和 JSONL 事件流，便于接入字幕生成、会议纪要、内容检索及其他自动化流程。
- 作为 Rust 库嵌入其他程序，直接消费统一的 PCM 音频帧，无需从命令行启动子进程。
- 枚举活跃音频应用，并在需要隔离时支持按应用名称选择。

### 核心优势

- **跨平台接口统一**：Windows 10、Windows 11 和 macOS 共用相同的 CLI 与 Rust API，平台差异由内部后端处理。
- **无需虚拟声卡**：直接使用 WASAPI 或 Core Audio，不要求用户安装虚拟音频设备或内核驱动，部署和卸载更简单。
- **默认即可使用**：不指定来源时，Windows 和 macOS 都直接捕获系统音频；只有需要隔离声音时才使用 `--app` 或 `--pid`。
- **适配多进程应用**：Windows 11 捕获目标进程及其子进程树，更适合浏览器、微信等由辅助进程实际播放音频的应用。
- **录音与识别解耦**：FunASR 完全可选，并可独立部署在另一台机器；没有 FunASR 时仍可发现应用和录制 WAV。
- **适合实时流水线**：音频格式转换和网络发送不在原生实时回调中执行，并使用有界队列限制内存增长。
- **故障可见**：捕获后端终止、网络断开、服务端错误和 ASR 发送队列积压都会明确返回错误，避免将故障误认为静音或成功完成。
- **输出便于集成**：既可保留无损的浮点 PCM WAV，也可直接获得纯文本和结构化 JSONL，方便后续编码、存储与业务处理。

### 构建

```shell
cargo build --release
```

构建产物位于 `target/release/`。macOS 14.2 以下版本不受支持。

### 普通用户快速使用

先在微信、浏览器、视频播放器或其他目标应用中开始播放声音。仅捕获音频并保存为 WAV 时，不需要 FunASR：

```shell
vapor-tap capture
```

不指定 `--seconds` 时，录音会持续运行。按一次 `Ctrl+C` 后，Vapor Tap 会停止捕获、刷新 WAV 数据并安全写入文件头，然后正常退出。需要固定时长时可增加 `--seconds 60`；定时录音也可以按 `Ctrl+C` 提前安全结束。

不指定 `--output` 时，文件会以本地时间自动命名为 `captured-YYYYMMDD-HHMMSS.wav`，保存在执行命令时的当前目录中；命令结束时会打印完整路径。例如，在 `E:\workspace\vapor-tap` 中执行，可能生成 `E:\workspace\vapor-tap\captured-20260824-153045.wav`。仍可使用 `--output recordings\meeting.wav` 指定文件名或路径。

Windows 10、Windows 11 和 macOS 都会在未指定来源时直接捕获系统音频，不显示应用选择菜单。系统通知和其他应用的声音也会被包含；需要隔离时再指定应用。

也可以先查看检测到的应用，再通过名称选择：

```shell
vapor-tap apps
vapor-tap transcribe --app WeChat --funasr-url ws://127.0.0.1:10095
vapor-tap capture --app Chrome --seconds 10 --output capture.wav
```

`--app` 支持按应用名称进行匹配。对于脚本和高级集成，仍可使用 PID：

```shell
vapor-tap capture --pid 1234 --seconds 10 --output capture.wav
```

在 Windows 10 上，即使指定了 `--app` 或 `--pid`，程序也会自动退化为默认输出设备的混合音频，并显示警告。Windows 11 会包含目标 PID 的子进程树；微信、浏览器等多进程应用的音频可能由辅助进程输出。

### 音频存储格式

`capture` 命令生成 WAV 文件，内容是捕获设备原始采样率和声道数下的交错 32 位 IEEE 浮点 PCM。WAV 本身是未压缩格式，便于后续处理且不会产生有损压缩。

`transcribe` 默认不在本地保存音频，只将处理后的音频发送给 FunASR。如需同时保留原始录音，可使用：

```shell
vapor-tap transcribe \
  --funasr-url ws://127.0.0.1:10095 \
  --save-audio original.wav
```

当前项目不直接输出 MP3。如有长期存储和空间要求，可在捕获完成后使用 FFmpeg 等工具将 WAV 转换为 MP3、AAC、Opus 或 FLAC；用于语音识别时，直接发送 PCM 可避免额外的编解码延迟和音质损失。

### 可选：连接远程 FunASR

只有需要将声音实时转换成文字时才需要 FunASR；单纯捕获或保存 WAV 不依赖 FunASR。FunASR 可以独立运行，也可以部署在另一台机器上。捕获端只作为 WebSocket 客户端，不需要安装 Python、Docker、模型或 GPU。

```shell
vapor-tap transcribe \
  --app WeChat \
  --funasr-url wss://asr.example.com/ws \
  --mode two-pass \
  --json-output transcript.jsonl
```

不指定 `--seconds` 时，转写会持续运行。按一次 `Ctrl+C` 后，Vapor Tap 会停止捕获、发送输入结束消息、等待 FunASR 返回最终结果，然后刷新并安全关闭文本、JSONL 和可选 WAV 文件。如需无人值守的固定时长任务，仍可使用 `--seconds 60` 等参数；即使指定了时长，也可以按 `Ctrl+C` 提前结束并正常收尾。

发送给 FunASR 前，音频会在工作线程中转换为单声道、16 kHz、有符号 PCM16 小端格式，并以 60 毫秒的二进制消息发送。默认识别模式为 `two-pass`：在线结果作为临时文本输出，`2pass-offline` 结果作为最终文本追加。

不指定 `--text-output` 时，最终识别文字会以本地时间自动保存为 `transcript-YYYYMMDD-HHMMSS.txt`，位置是执行命令时的当前目录，完成后会打印完整路径。可使用 `--text-output records\meeting.txt` 自定义文件名或路径。`--json-output` 仍是可选的 JSONL 事件流，会区分 `partial`、`final`、`server_error` 和 `disconnected` 事件。连接断开时命令会明确报错并退出，重新启动命令会创建新的识别会话。

如果服务需要 Bearer Token，请通过环境变量传入，避免令牌出现在命令行历史中：

```powershell
$env:VAPOR_TAP_FUNASR_TOKEN = "secret"
vapor-tap transcribe --funasr-url wss://asr.example.com/ws
```

通过不受信任的网络访问远程服务时应使用 `wss://`。
`wss://` 服务必须提供操作系统信任且与访问地址匹配的证书。Vapor Tap 不会跳过 TLS 证书校验；使用自签名证书时，应将签发它的 CA 安装到客户端系统的信任库，或在可信的反向代理上终止 TLS。

### macOS 权限与打包

macOS 可执行文件应放入已签名的应用包中，并在 `Info.plist` 中配置：

```xml
<key>LSMinimumSystemVersion</key>
<string>14.2</string>
<key>NSAudioCaptureUsageDescription</key>
<string>Capture system or application audio selected by the user.</string>
```

用户需要在“系统设置 → 隐私与安全性 → 屏幕与系统音频录制”中授予权限。程序停止或 `CaptureSession` 被释放时，会清理创建的全局/进程 Tap 和聚合设备。

### 当前验证状态

- Windows 10 build 19045：已在本机完成编译、单元测试和实际回环调用，但最近一次保存的 WAV 全部为零采样；真实非静音捕获仍需继续排查，不能视为已验证。
- Windows 11：代码已完成编译和单元测试，仍需在 Windows 11 真机上完成按 PID 捕获的最终验证。
- macOS：全局与进程 Tap 已通过 `aarch64-apple-darwin` 目标交叉检查，权限弹窗和真实非零音频仍需在 macOS 14.2 或更高版本的真机上验证。
- FunASR：已使用实际远程模型服务验证 WebSocket 上行、在线/离线结果和最终结束确认。测试服务使用不受系统信任的自签名证书，因此测试时通过临时 TLS 代理连接；最近一次 Windows 捕获为全零 PCM，识别内容属于模型对静音的错误输出，尚不能作为识别质量验证。

---

## English documentation

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
