#Requires -Version 5.1

<#
.SYNOPSIS
Prepares a local FunASR service and optionally starts Vapor Tap.

.DESCRIPTION
Checks Docker Desktop, pulls the pinned FunASR online CPU image, starts a
persistent local-only container, and waits for ws://127.0.0.1:10095.

.PARAMETER InstallDocker
Install Docker Desktop with winget when Docker is missing. Installation may
require elevation, accepting Docker's terms, and a Windows restart.

.PARAMETER StartVaporTap
Start Vapor Tap transcription after FunASR is ready.

.EXAMPLE
.\setup-funasr.ps1 -InstallDocker -StartVaporTap

.EXAMPLE
.\setup-funasr.ps1 -StartVaporTap -App WeChat -Seconds 60
#>

[CmdletBinding()]
param(
    [switch] $InstallDocker,
    [switch] $StartVaporTap,
    [string] $App,
    [ValidateRange(0, 2147483647)]
    [int] $Seconds = 0,
    [string] $VaporTapPath,
    [ValidateRange(30, 86400)]
    [int] $StartupTimeoutSeconds = 1800
)

$ErrorActionPreference = 'Stop'
$image = 'registry.cn-hangzhou.aliyuncs.com/funasr_repo/funasr:funasr-runtime-sdk-online-cpu-0.1.13'
$containerName = 'vapor-tap-funasr'
$port = 10095
$dockerCli = Join-Path $env:ProgramFiles 'Docker\Docker\resources\bin\docker.exe'
$dockerDesktop = Join-Path $env:ProgramFiles 'Docker\Docker\Docker Desktop.exe'

$dockerCommand = Get-Command docker -ErrorAction SilentlyContinue
if ($null -eq $dockerCommand -and -not (Test-Path -LiteralPath $dockerCli -PathType Leaf)) {
    if (-not $InstallDocker) {
        throw 'Docker Desktop is not installed. Rerun with -InstallDocker, or install it manually first.'
    }

    $winget = Get-Command winget -ErrorAction SilentlyContinue
    if ($null -eq $winget) {
        throw 'winget is unavailable. Install Docker Desktop from https://docs.docker.com/desktop/setup/install/windows-install/ and rerun this script.'
    }

    Write-Host 'Installing Docker Desktop with winget. Windows may request administrator approval.'
    & $winget.Source install --exact --id Docker.DockerDesktop --accept-package-agreements --accept-source-agreements
    if ($LASTEXITCODE -ne 0) {
        throw "Docker Desktop installation failed with exit code $LASTEXITCODE."
    }
}

$dockerCommand = Get-Command docker -ErrorAction SilentlyContinue
if ($null -ne $dockerCommand) {
    $dockerPath = $dockerCommand.Source
}
elseif (Test-Path -LiteralPath $dockerCli -PathType Leaf) {
    $dockerPath = $dockerCli
}
else {
    throw 'Docker Desktop was installed but docker.exe is not available yet. Restart Windows, then rerun this script.'
}

function Get-DockerOsType {
    $value = & $dockerPath info --format '{{.OSType}}' 2>$null
    if ($LASTEXITCODE -eq 0) {
        return "$value".Trim()
    }
    return $null
}

$dockerOsType = Get-DockerOsType
if ($dockerOsType -ne 'linux') {
    if (Test-Path -LiteralPath $dockerDesktop -PathType Leaf) {
        Write-Host 'Starting Docker Desktop...'
        Start-Process -FilePath $dockerDesktop | Out-Null
    }

    $dockerDeadline = [DateTime]::UtcNow.AddSeconds(180)
    do {
        Start-Sleep -Seconds 3
        $dockerOsType = Get-DockerOsType
    } while ($dockerOsType -ne 'linux' -and $dockerOsType -ne 'windows' -and [DateTime]::UtcNow -lt $dockerDeadline)
}

if ($dockerOsType -eq 'windows') {
    throw 'Docker Desktop is using Windows containers. Switch it to Linux containers and rerun this script.'
}
if ($dockerOsType -ne 'linux') {
    throw 'Docker Desktop did not become ready within 180 seconds. Complete its first-run setup or restart Windows, then rerun this script.'
}

$modelsDirectory = Join-Path $env:LOCALAPPDATA 'VaporTap\FunASR\models'
New-Item -ItemType Directory -Path $modelsDirectory -Force | Out-Null

$containerStatus = & $dockerPath container inspect --format '{{.State.Status}}' $containerName 2>$null
if ($LASTEXITCODE -eq 0) {
    if ("$containerStatus".Trim() -ne 'running') {
        Write-Host "Starting existing $containerName container..."
        & $dockerPath start $containerName
        if ($LASTEXITCODE -ne 0) {
            throw "Could not start $containerName. Remove or rename that container, then rerun this script."
        }
    }
    else {
        Write-Host "$containerName is already running."
    }
}
else {
    Write-Host "Pulling $image ..."
    & $dockerPath pull $image
    if ($LASTEXITCODE -ne 0) {
        throw "Could not pull the FunASR image (exit code $LASTEXITCODE)."
    }

    $serverCommand = @'
set -e
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
trap 'kill -TERM "$server_pid" 2>/dev/null || true; wait "$server_pid"' TERM INT
wait "$server_pid"
'@

    $mount = "type=bind,source=$modelsDirectory,target=/workspace/models"
    Write-Host "Creating $containerName with model cache $modelsDirectory ..."
    & $dockerPath run --detach --name $containerName --restart unless-stopped `
        --publish "127.0.0.1:${port}:10095" `
        --mount $mount --entrypoint bash $image -lc $serverCommand
    if ($LASTEXITCODE -ne 0) {
        throw "Could not create $containerName (exit code $LASTEXITCODE)."
    }
}

function Test-FunAsrPort {
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $task = $client.ConnectAsync('127.0.0.1', $port)
        return $task.Wait(1000) -and $client.Connected
    }
    catch {
        return $false
    }
    finally {
        $client.Dispose()
    }
}

Write-Host "Waiting for FunASR on ws://127.0.0.1:$port ..."
Write-Host 'The first start downloads and loads the models, which can take several minutes.'
$funAsrDeadline = [DateTime]::UtcNow.AddSeconds($StartupTimeoutSeconds)
$nextLogTime = [DateTime]::UtcNow.AddSeconds(30)
while (-not (Test-FunAsrPort)) {
    $containerStatus = & $dockerPath container inspect --format '{{.State.Status}}' $containerName 2>$null
    if ($LASTEXITCODE -ne 0 -or "$containerStatus".Trim() -ne 'running') {
        & $dockerPath logs --tail 100 $containerName
        throw "$containerName stopped before FunASR became ready."
    }
    if ([DateTime]::UtcNow -ge $funAsrDeadline) {
        & $dockerPath logs --tail 100 $containerName
        throw "FunASR did not open port $port within $StartupTimeoutSeconds seconds."
    }
    if ([DateTime]::UtcNow -ge $nextLogTime) {
        Write-Host 'FunASR is still initializing. Recent output:'
        & $dockerPath logs --tail 8 $containerName
        $nextLogTime = [DateTime]::UtcNow.AddSeconds(30)
    }
    Start-Sleep -Seconds 3
}

Write-Host "FunASR is ready: ws://127.0.0.1:$port"
Write-Host "Model cache: $modelsDirectory"
if (-not $StartVaporTap) {
    Write-Host "Run Vapor Tap with: vapor-tap transcribe --funasr-url ws://127.0.0.1:$port"
    return
}

if (-not [string]::IsNullOrWhiteSpace($VaporTapPath)) {
    $resolvedVaporTap = (Resolve-Path -LiteralPath $VaporTapPath).ProviderPath
}
else {
    $resolvedVaporTap = @(
        (Join-Path $PSScriptRoot 'vapor-tap.exe'),
        (Join-Path $PSScriptRoot 'target\release\vapor-tap.exe')
    ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
    if ($null -eq $resolvedVaporTap) {
        $vaporTapCommand = Get-Command vapor-tap.exe -ErrorAction SilentlyContinue
        if ($null -ne $vaporTapCommand) {
            $resolvedVaporTap = $vaporTapCommand.Source
        }
    }
}
if ([string]::IsNullOrWhiteSpace($resolvedVaporTap)) {
    throw 'vapor-tap.exe was not found. Put it beside this script, build it under target\release, or pass -VaporTapPath.'
}

$vaporTapArguments = @('transcribe', '--funasr-url', "ws://127.0.0.1:$port")
if (-not [string]::IsNullOrWhiteSpace($App)) {
    $vaporTapArguments += @('--app', $App)
}
if ($Seconds -gt 0) {
    $vaporTapArguments += @('--seconds', "$Seconds")
}

Write-Host "Starting $resolvedVaporTap $($vaporTapArguments -join ' ')"
& $resolvedVaporTap @vaporTapArguments
if ($LASTEXITCODE -ne 0) {
    throw "Vapor Tap exited with code $LASTEXITCODE."
}
