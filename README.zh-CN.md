# vapor-tap

[English](README.md) | 简体中文

Vapor Tap 是一个使用 Rust 实现的跨平台应用音频捕获工具，可独立捕获声音并保存为 WAV。FunASR 只是可选的语音转文字后端：不安装、不配置或不连接 FunASR，也可以正常完成应用发现、音频捕获和本地录音。Vapor Tap 不需要安装虚拟声卡或内核驱动。

支持的平台和捕获方式：

- Windows 10 22H2（build 19045）：默认通过 WASAPI 回环捕获系统混合音频。该系统不支持按进程隔离；即使指定应用，也会退化为系统混音。
- Windows 11（build 20348 或更高版本）：默认通过 WASAPI 捕获系统混合音频；使用 `--app` 或 `--pid` 时，通过进程回环隔离目标应用及其子进程。
- macOS 14.2 或更高版本：默认通过 Core Audio 全局 Tap 捕获系统音频；使用 `--app` 或 `--pid` 时改用进程 Tap。

公共 Rust API 与平台无关，输出交错排列的 `f32` PCM 音频数据。

## 项目作用

Vapor Tap 在桌面应用与音频处理服务之间提供一条轻量、可编程的音频通道，适合以下场景：

- 捕获微信、浏览器、视频播放器、会议软件等应用正在播放的声音，用于录音、归档或后续分析。
- 将音频实时发送给本机或远程 FunASR，把会议、课程、视频和通话内容转换成文字。
- 同时输出 WAV 原始录音、纯文本和 JSONL 事件流，便于接入字幕生成、会议纪要、内容检索及其他自动化流程。
- 作为 Rust 库嵌入其他程序，直接消费统一的 PCM 音频帧，无需从命令行启动子进程。
- 枚举活跃音频应用，并在需要隔离时支持按应用名称选择。

## 核心优势

- **跨平台接口统一**：Windows 10、Windows 11 和 macOS 共用相同的 CLI 与 Rust API，平台差异由内部后端处理。
- **无需虚拟声卡**：直接使用 WASAPI 或 Core Audio，不要求用户安装虚拟音频设备或内核驱动，部署和卸载更简单。
- **默认即可使用**：不指定来源时，Windows 和 macOS 都直接捕获系统音频；只有需要隔离声音时才使用 `--app` 或 `--pid`。
- **适配多进程应用**：Windows 11 捕获目标进程及其子进程树，更适合浏览器、微信等由辅助进程实际播放音频的应用。
- **录音与识别解耦**：FunASR 完全可选，并可独立部署在另一台机器；没有 FunASR 时仍可发现应用和录制 WAV。
- **适合实时流水线**：音频格式转换和网络发送不在原生实时回调中执行，并使用有界队列限制内存增长。
- **故障可见**：捕获后端终止、网络断开、服务端错误和 ASR 发送队列积压都会明确返回错误，避免将故障误认为静音或成功完成。
- **输出便于集成**：既可保留无损的浮点 PCM WAV，也可直接获得纯文本和结构化 JSONL，方便后续编码、存储与业务处理。

## 构建

```shell
cargo build --release
```

构建产物位于 `target/release/`。macOS 14.2 以下版本不受支持。

## 普通用户快速使用

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

## 音频存储格式

`capture` 命令生成 WAV 文件，内容是捕获设备原始采样率和声道数下的交错 32 位 IEEE 浮点 PCM。WAV 本身是未压缩格式，便于后续处理且不会产生有损压缩。

`transcribe` 默认不在本地保存音频，只将处理后的音频发送给 FunASR。如需同时保留原始录音，可使用：

```shell
vapor-tap transcribe \
  --funasr-url ws://127.0.0.1:10095 \
  --save-audio original.wav
```

当前项目不直接输出 MP3。如有长期存储和空间要求，可在捕获完成后使用 FFmpeg 等工具将 WAV 转换为 MP3、AAC、Opus 或 FLAC；用于语音识别时，直接发送 PCM 可避免额外的编解码延迟和音质损失。

## 可选：连接远程 FunASR

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

## 可选：离线安装 FunASR 服务

这里的“离线安装”是指将 FunASR 部署到无法访问互联网的机器，不是
FunASR 协议中的 `offline` 识别模式。Vapor Tap 默认使用 `two-pass`，因此应准备
FunASR 的在线/两遍 WebSocket 运行时；不要误用只提供离线文件转写的运行时镜像。

下面以官方 CPU Docker 镜像为例。命令在 Linux 或其他支持 Linux 容器的
POSIX 环境中执行；Vapor Tap 本身可以运行在 Windows 或 macOS，并通过网络连接
这台服务端。离线机器需要预先安装 Docker Engine，且联网准备机器应与离线机器
使用相同的 CPU 架构。镜像版本来自
[FunASR 官方实时服务部署文档](https://github.com/modelscope/FunASR/blob/main/runtime/docs/SDK_advanced_guide_online.md)；升级版本时应同时重新准备镜像和模型。

先在可联网的机器上拉取镜像，并让服务启动一次以下载全部模型：

```shell
FUNASR_IMAGE=registry.cn-hangzhou.aliyuncs.com/funasr_repo/funasr:funasr-runtime-sdk-online-cpu-0.1.13

mkdir -p funasr-airgap/models
docker pull "$FUNASR_IMAGE"
docker run --rm -it --entrypoint bash -p 10095:10095 \
  -v "$PWD/funasr-airgap/models:/workspace/models" \
  "$FUNASR_IMAGE"
```

进入容器后运行：

```shell
cd /workspace/FunASR/runtime
bash run_server_2pass.sh \
  --download-model-dir /workspace/models \
  --certfile 0 >log.txt 2>&1
tail -f log.txt
```

等待日志显示所有模型加载完成并开始监听 `10095` 端口。另开终端确认
`funasr-airgap/models` 不为空；随后按 `Ctrl+C` 停止查看日志，退出容器。模型位于
宿主机目录中，不会随 `--rm` 删除。将镜像、模型和校验文件打包：

```shell
docker save -o funasr-image.tar "$FUNASR_IMAGE"
tar -C funasr-airgap -czf funasr-models.tar.gz models
sha256sum funasr-image.tar funasr-models.tar.gz > SHA256SUMS
```

把 `funasr-image.tar`、`funasr-models.tar.gz` 和 `SHA256SUMS` 复制到离线机器，
校验后导入：

```shell
sha256sum -c SHA256SUMS
docker load -i funasr-image.tar
mkdir -p funasr-runtime-resources
tar -C funasr-runtime-resources -xzf funasr-models.tar.gz
```

在离线机器启动容器：

```shell
FUNASR_IMAGE=registry.cn-hangzhou.aliyuncs.com/funasr_repo/funasr:funasr-runtime-sdk-online-cpu-0.1.13

docker run --rm -it --entrypoint bash --name funasr -p 10095:10095 \
  -v "$PWD/funasr-runtime-resources/models:/workspace/models" \
  "$FUNASR_IMAGE"
```

进入容器后使用本地路径启动，避免服务尝试访问 ModelScope：

```shell
cd /workspace/FunASR/runtime
bash run_server_2pass.sh \
  --download-model-dir /workspace/models \
  --model-dir /workspace/models/damo/speech_paraformer-large-vad-punc_asr_nat-zh-cn-16k-common-vocab8404-onnx \
  --online-model-dir /workspace/models/damo/speech_paraformer-large_asr_nat-zh-cn-16k-common-vocab8404-online-onnx \
  --vad-dir /workspace/models/damo/speech_fsmn_vad_zh-cn-16k-common-onnx \
  --punc-dir /workspace/models/damo/punc_ct-transformer_zh-cn-common-vad_realtime-vocab272727-onnx \
  --lm-dir /workspace/models/damo/speech_ngram_lm_zh-cn-ai-wesp-fst \
  --itn-dir /workspace/models/thuduj12/fst_itn_zh \
  --certfile 0 >log.txt 2>&1
tail -f log.txt
```

本机测试时连接 `ws://127.0.0.1:10095`；FunASR 在另一台局域网机器上时，将
`127.0.0.1` 换成服务端地址，并只在可信网络中直接使用 `ws://`：

```shell
vapor-tap transcribe --funasr-url ws://127.0.0.1:10095 --seconds 10
```

如果日志提示某个本地模型目录不存在，说明联网机器上的预下载没有完成；重新联网
启动服务并等待全部模型加载成功后，再重新打包模型目录。Vapor Tap 捕获端无需安装
FunASR、Python 或模型。

## macOS 权限与打包

macOS 可执行文件应放入已签名的应用包中，并在 `Info.plist` 中配置：

```xml
<key>LSMinimumSystemVersion</key>
<string>14.2</string>
<key>NSAudioCaptureUsageDescription</key>
<string>Capture system or application audio selected by the user.</string>
```

用户需要在“系统设置 → 隐私与安全性 → 屏幕与系统音频录制”中授予权限。程序停止或 `CaptureSession` 被释放时，会清理创建的全局/进程 Tap 和聚合设备。

## 当前验证状态

- Windows 10 build 19045：已在本机完成编译、单元测试和实际回环调用，但最近一次保存的 WAV 全部为零采样；真实非静音捕获仍需继续排查，不能视为已验证。
- Windows 11：代码已完成编译和单元测试，仍需在 Windows 11 真机上完成按 PID 捕获的最终验证。
- macOS：全局与进程 Tap 已通过 `aarch64-apple-darwin` 目标交叉检查，权限弹窗和真实非零音频仍需在 macOS 14.2 或更高版本的真机上验证。
- FunASR：已使用实际远程模型服务验证 WebSocket 上行、在线/离线结果和最终结束确认。测试服务使用不受系统信任的自签名证书，因此测试时通过临时 TLS 代理连接；最近一次 Windows 捕获为全零 PCM，识别内容属于模型对静音的错误输出，尚不能作为识别质量验证。
