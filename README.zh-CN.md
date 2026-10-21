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

## Windows 一键启动本地 FunASR

[`setup-funasr.ps1`](setup-funasr.ps1) 会检查 Docker Desktop、拉取固定版本的
FunASR 在线 CPU 镜像、下载必需模型、启动可长期复用的容器，并等待
`ws://127.0.0.1:10095` 可用。如果尚未安装 Docker Desktop，可通过 `winget`
安装并在服务就绪后直接启动 Vapor Tap：

```powershell
powershell -ExecutionPolicy Bypass -File .\setup-funasr.ps1 -InstallDocker -StartVaporTap
```

已经安装 Docker Desktop 时去掉 `-InstallDocker`。例如，只转写微信 60 秒：

```powershell
.\setup-funasr.ps1 -StartVaporTap -App WeChat -Seconds 60
```

不指定 `-StartVaporTap` 时只准备并启动服务。模型默认保存在
`%LOCALAPPDATA%\VaporTap\FunASR\models`，以后运行会复用
`vapor-tap-funasr` 容器；未加密的 WebSocket 只绑定到本机
`127.0.0.1`。首次下载和加载模型可能需要数分钟。Docker Desktop 安装可能要求
管理员确认、接受其许可条款或重启 Windows，脚本会明确提示这些情况，不会绕过系统
要求。

```powershell
docker logs -f vapor-tap-funasr
docker stop vapor-tap-funasr
docker start vapor-tap-funasr
```

脚本会依次查找同目录下的 `vapor-tap.exe`、`target\release` 中的构建产物以及
`PATH`；也可以使用 `-VaporTapPath C:\path\to\vapor-tap.exe` 明确指定。镜像和
模型列表来自
[FunASR 官方实时服务部署文档](https://github.com/modelscope/FunASR/blob/main/runtime/docs/SDK_advanced_guide_online.md)。

## FunASR 硬件要求

本项目使用的 `funasr-runtime-sdk-online-cpu` 服务不要求 GPU。根据
[FunASR 官方实时部署教程](https://github.com/modelscope/FunASR/blob/main/runtime/docs/SDK_tutorial_online_zh.md)，
官方针对 x86 计算型服务器给出了以下推荐配置：

| CPU | 内存 | 估算并发路数 |
| --- | ---: | ---: |
| 4 核 vCPU | 8 GB | 约 16 路 |
| 16 核 vCPU | 32 GB | 约 32 路 |
| 64 核 vCPU | 128 GB | 约 100 路 |

这些是推荐规格和近似容量，不是强制最低配置。实际容量还取决于 CPU 性能、模型、
音频负载和线程配置。对于单路 Vapor Tap 实时转写，可将 Linux 4 核 CPU、8 GB
内存作为实用起点；多路并发或长期生产运行建议使用 8 核、16 GB 或更高配置。

当前 CPU 镜像支持 AMD64 和 ARM64，但上表的官方容量数据基于 x86 服务器。
便捷部署脚本仅支持 Linux；Windows 和 macOS 可以运行 Vapor Tap，再连接 Linux
主机或 Linux 容器中的 FunASR 服务。官方未给出固定的最低磁盘空间。为容纳 Docker
镜像、离线与流式 ASR 模型、VAD、标点、语言模型、ITN 资源及日志，建议先预留
至少 15–20 GB，再以实际下载后的文件大小为准。

服务端默认根据可用 CPU 自动计算解码和 I/O 线程数。手工调优时，FunASR 建议让
`decoder-thread-num × model-thread-num` 接近 CPU 总线程数。其他基于 GPU 的
FunASR 运行时有独立的 CUDA 和显存要求，但本项目的当前集成不需要 GPU。

## 可选：离线安装 FunASR 服务

这里的“离线安装”是指将 FunASR 部署到无法访问互联网的机器，不是
FunASR 协议中的 `offline` 识别模式。Vapor Tap 默认使用 `two-pass`，因此应准备
FunASR 的在线/两遍 WebSocket 运行时；不要误用只提供离线文件转写的运行时镜像。

下面以官方 CPU Docker 镜像为例。命令在 Linux 或其他支持 Linux 容器的
POSIX 环境中执行；Vapor Tap 本身可以运行在 Windows 或 macOS，并通过网络连接
这台服务端。离线机器需要预先安装 Docker Engine，且联网准备机器应与离线机器
使用相同的 CPU 架构。镜像版本来自
[FunASR 官方实时服务部署文档](https://github.com/modelscope/FunASR/blob/main/runtime/docs/SDK_advanced_guide_online.md)；升级版本时应同时重新准备镜像和模型。

### 1. 在联网机器下载镜像和模型

拉取固定版本的镜像、创建宿主机模型目录，并进入临时预下载容器的 Bash。下面的
命令只是进入容器，还没有启动 FunASR 服务：

```shell
FUNASR_IMAGE=registry.cn-hangzhou.aliyuncs.com/funasr_repo/funasr:funasr-runtime-sdk-online-cpu-0.1.13

mkdir -p funasr-airgap/models
docker pull "$FUNASR_IMAGE"
docker run --rm -it --name funasr-prefetch \
  --entrypoint bash -p 10095:10095 \
  -v "$PWD/funasr-airgap/models:/workspace/models" \
  "$FUNASR_IMAGE"
```

进入容器后，显式指定全部模型并启动服务。联网预下载和离线启动使用相同的模型
列表，可避免镜像默认值发生变化后下载到不同的模型：

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

官方脚本会把 FunASR 服务放到后台运行。这里使用 `.` 加载脚本，使服务进程仍属于
当前 Shell；`wait` 会保持容器运行，服务日志则直接显示在当前终端。

### 2. 验证并打包离线资源

保持预下载容器运行，在联网机器的另一个终端检查六个必需模型目录是否全部存在：

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

确认服务日志没有下载或模型加载错误，并且已监听 `10095` 端口。播放一段包含语音的
音频，再从能够访问联网服务端的机器执行一次端到端测试：

```shell
vapor-tap transcribe --funasr-url ws://127.0.0.1:10095 --seconds 10
```

如果 Vapor Tap 不在服务端本机运行，将 `127.0.0.1` 换成联网服务端地址。测试成功
后，在第二个终端停止临时容器：

```shell
docker stop funasr-prefetch
```

模型目录挂载在宿主机上，因此 `--rm` 只会删除临时容器，不会删除模型。随后导出
固定版本的镜像、已验证模型和校验文件：

```shell
docker save -o funasr-image.tar "$FUNASR_IMAGE"
tar -C funasr-airgap -czf funasr-models.tar.gz models
sha256sum funasr-image.tar funasr-models.tar.gz > SHA256SUMS
```

把 `funasr-image.tar`、`funasr-models.tar.gz` 和 `SHA256SUMS` 复制到离线机器。

### 3. 在离线机器导入并验证

```shell
sha256sum -c SHA256SUMS
docker load -i funasr-image.tar
mkdir -p funasr-runtime-resources
tar -C funasr-runtime-resources -xzf funasr-models.tar.gz
```

校验和用于确认传输后的归档没有损坏。也可以重复上一步的目录检查，并将检查路径
改成 `funasr-runtime-resources/models/$model`。

### 4. 进行一次性交互验证

下面的命令会创建一次性容器并进入 Bash，仅用于首次验证；退出后容器会被删除：

```shell
FUNASR_IMAGE=registry.cn-hangzhou.aliyuncs.com/funasr_repo/funasr:funasr-runtime-sdk-online-cpu-0.1.13

docker run --rm -it --name funasr-smoke \
  --entrypoint bash -p 10095:10095 \
  -v "$PWD/funasr-runtime-resources/models:/workspace/models" \
  "$FUNASR_IMAGE"
```

进入容器后显式使用本地模型路径，避免运行时尝试解析 ModelScope 模型 ID：

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

从另一个终端执行 Vapor Tap 测试。完成后，可在容器终端按 `Ctrl+C`，或从第二个
终端执行 `docker stop funasr-smoke`。

### 5. 后台长期运行 FunASR

交互验证通过后，创建不带 `--rm` 的后台容器。重启策略会在宿主机或 Docker
重启后重新启动服务：

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

使用以下命令查看日志和管理后台服务：

```shell
docker logs -f funasr
docker stop funasr
docker start funasr
```

本机客户端连接 `ws://127.0.0.1:10095`；FunASR 位于另一台局域网机器时，改用
服务端地址，并只在可信网络中直接使用 `ws://`。Vapor Tap 捕获端无需安装
FunASR、Python 或模型。如果缺少任何本地模型目录，应回到联网机器完成下载和
端到端测试，再重新制作离线包。

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

- Windows 10 build 19045：已在本机完成编译和单元测试。2026-10-21 使用系统 WAV 播放音频并进行 10 秒 WASAPI 系统回环录制，共得到 479,040 个 48 kHz 双声道音频帧；958,080 个采样中有 743,179 个（77.57%）绝对值大于 `1e-6`，峰值为 0.54669，RMS 为 0.04527。这次测试确认该机器能够捕获非零的系统混音音频。
- Windows 11：代码已完成编译和单元测试，仍需在 Windows 11 真机上完成按 PID 捕获的最终验证。
- macOS：全局与进程 Tap 已通过 `aarch64-apple-darwin` 目标交叉检查，权限弹窗和真实非零音频仍需在 macOS 14.2 或更高版本的真机上验证。
- FunASR：已使用实际远程模型服务验证 WebSocket 上行、在线/离线结果和最终结束确认。测试服务使用不受系统信任的自签名证书，因此测试时通过临时 TLS 代理连接；本次非零 Windows 录音尚未发送给真实 FunASR 服务，端到端识别质量仍未完成验证。
