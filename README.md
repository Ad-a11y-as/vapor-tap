# vapor-tap

## 中文说明

Vapor Tap 是一个使用 Rust 实现的跨平台应用音频捕获工具，可独立捕获声音并保存为 WAV。FunASR 只是可选的语音转文字后端：不安装、不配置或不连接 FunASR，也可以正常完成应用发现、音频捕获和本地录音。Vapor Tap 不需要安装虚拟声卡或内核驱动。

支持的平台和捕获方式：

- Windows 10 22H2（build 19045）：通过 WASAPI 回环捕获默认输出设备的混合音频。由于该系统不支持按进程隔离音频，因此无需选择应用；使用时应尽量保证只有目标应用在播放声音。
- Windows 11（build 20348 或更高版本）：通过 WASAPI 进程回环捕获指定应用及其子进程的音频。
- macOS 14.2 或更高版本：通过 Core Audio 进程 Tap 捕获指定应用的音频。

公共 Rust API 与平台无关，输出交错排列的 `f32` PCM 音频数据。

### 构建

```shell
cargo build --release
```

构建产物位于 `target/release/`。macOS 14.2 以下版本不受支持。

### 普通用户快速使用

先在微信、浏览器、视频播放器或其他目标应用中开始播放声音。仅捕获音频并保存为 WAV 时，不需要 FunASR：

```shell
vapor-tap capture --seconds 10 --output capture.wav
```

不同系统的行为如下：

- Windows 10：直接捕获默认输出设备的全部混合音频，不显示应用选择菜单。
- Windows 11 和 macOS：自动检测当前正在输出声音的应用，并显示编号菜单供用户选择，不需要用户查找 PID。

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

Windows 还支持查看和指定输出设备：

```shell
vapor-tap devices
vapor-tap capture --default-device --seconds 10 --output capture.wav
vapor-tap capture --device "Speakers (Realtek Audio)" --seconds 10 --output capture.wav
```

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
  --seconds 60 \
  --funasr-url wss://asr.example.com/ws \
  --mode two-pass \
  --text-output transcript.txt \
  --json-output transcript.jsonl
```

发送给 FunASR 前，音频会在工作线程中转换为单声道、16 kHz、有符号 PCM16 小端格式，并以 60 毫秒的二进制消息发送。默认识别模式为 `two-pass`：在线结果作为临时文本输出，`2pass-offline` 结果作为最终文本追加。

`--text-output` 保存纯文本，`--json-output` 保存 JSONL 事件流。JSONL 会区分 `partial`、`final`、`server_error` 和 `disconnected` 事件。连接断开时命令会明确报错并退出，重新启动命令会创建新的识别会话。

如果服务需要 Bearer Token，请通过环境变量传入，避免令牌出现在命令行历史中：

```powershell
$env:VAPOR_TAP_FUNASR_TOKEN = "secret"
vapor-tap transcribe --funasr-url wss://asr.example.com/ws
```

通过不受信任的网络访问远程服务时应使用 `wss://`。

### macOS 权限与打包

macOS 可执行文件应放入已签名的应用包中，并在 `Info.plist` 中配置：

```xml
<key>LSMinimumSystemVersion</key>
<string>14.2</string>
<key>NSAudioCaptureUsageDescription</key>
<string>Capture audio from the application selected by the user.</string>
```

用户需要在“系统设置 → 隐私与安全性 → 屏幕与系统音频录制”中授予权限。程序停止或 `CaptureSession` 被释放时，会清理创建的进程 Tap 和聚合设备。

### 当前验证状态

- Windows 10 build 19045：已在本机使用真实 Realtek 输出设备完成编译、单元测试和实际回环捕获，能够获取非静音 PCM；应用发现、`--app` 捕获以及不指定来源时的自动混音捕获均已验证。
- Windows 11：代码已完成编译和单元测试，仍需在 Windows 11 真机上完成按 PID 捕获的最终验证。
- macOS：已通过 `aarch64-apple-darwin` 目标交叉检查，权限弹窗和真实非零音频仍需在 macOS 14.2 或更高版本的真机上验证。
- FunASR：WebSocket 协议已通过本地模拟服务测试；仍需使用实际 FunASR 模型服务进行端到端验证。

---

## English documentation

Cross-platform application audio capture for:

- Windows 10 (default output mix through WASAPI loopback)
- Windows 11 (WASAPI process loopback)
- macOS 14.2 or newer (Core Audio process taps)

The public Rust API is platform-neutral and produces interleaved `f32` PCM.
No virtual audio device or kernel driver is required.
FunASR is an optional transcription backend; application discovery, capture,
and WAV recording work without installing or connecting to FunASR.

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
vapor-tap capture --seconds 10 --output capture.wav
```

To transcribe instead of only recording, connect the optional FunASR backend:

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

## Optional remote FunASR transcription

FunASR is required only for live speech-to-text. Audio capture and WAV recording
do not depend on it. FunASR runs independently and may be on another machine.
Vapor Tap connects as a WebSocket client; the capture machine does not need
Python, Docker, models, or a GPU.

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
