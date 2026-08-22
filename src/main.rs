use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::json;
use vapor_tap::asr::{AsrMode, FunAsrClient, FunAsrConfig, FunAsrEventReceiver, TranscriptEvent};
use vapor_tap::audio::SpeechNormalizer;
use vapor_tap::{
    AudioApplication, CaptureConfig, CaptureMode, CaptureSession, CaptureSource, Error, Result,
    WavWriter, list_audio_applications, resolve_audio_application,
};

#[derive(Parser, Debug)]
#[command(version, about = "Capture or transcribe application audio")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List applications that currently have a running audio output stream.
    Apps,
    /// Capture application or output-mix audio to an IEEE-float WAV file.
    Capture(CaptureArgs),
    /// Stream application or output-mix audio to a remote FunASR WebSocket service.
    Transcribe(TranscribeArgs),
}

#[derive(Args, Debug)]
struct CaptureArgs {
    #[command(flatten)]
    source: SourceArgs,
    /// Capture duration in seconds.
    #[arg(long, default_value_t = 10)]
    seconds: u64,
    /// Destination IEEE-float WAV file.
    #[arg(long, default_value = "capture.wav")]
    output: PathBuf,
}

#[derive(Args, Debug)]
struct TranscribeArgs {
    #[command(flatten)]
    source: SourceArgs,
    /// Optional capture duration in seconds. Omit to run until Ctrl+C.
    #[arg(long)]
    seconds: Option<u64>,
    /// Remote FunASR WebSocket URL (ws:// or wss://).
    #[arg(long)]
    funasr_url: String,
    /// Recognition mode. Two-pass gives partial text followed by corrected final text.
    #[arg(long, value_enum, default_value_t = ModeArg::TwoPass)]
    mode: ModeArg,
    /// Optional JSON hotword object, for example: {"Vapor Tap":20}.
    #[arg(long)]
    hotwords: Option<String>,
    /// Environment variable containing the bearer token. The token is not exposed in argv.
    #[arg(long, default_value = "VAPOR_TAP_FUNASR_TOKEN")]
    bearer_token_env: String,
    /// Write final sentences to this UTF-8 text file.
    #[arg(long)]
    text_output: Option<PathBuf>,
    /// Write partial/final/error events to this JSON Lines file.
    #[arg(long)]
    json_output: Option<PathBuf>,
    /// Optionally retain the original native audio as float WAV while transcribing.
    #[arg(long)]
    save_audio: Option<PathBuf>,
    /// FunASR audio queue capacity in 60 ms chunks.
    #[arg(long, default_value_t = 128)]
    queue_capacity: usize,
}

#[derive(Args, Debug)]
#[group(multiple = false)]
struct SourceArgs {
    /// Advanced target PID. Windows 10 ignores it and captures the default output mix.
    #[arg(long, value_name = "PID")]
    pid: Option<u32>,
    /// Isolate a playing app by name on Windows 11/macOS. Windows 10 falls back to system audio.
    #[arg(long, value_name = "QUERY")]
    app: Option<String>,
    /// Deprecated compatibility alias; system audio is already the default.
    #[arg(long = "default-device", hide = true)]
    _default_device: bool,
}

impl SourceArgs {
    fn capture_config(&self) -> Result<CaptureConfig> {
        if let Some(pid) = self.pid {
            return Ok(CaptureConfig::for_pid(pid));
        }
        if let Some(query) = &self.app {
            let application = resolve_audio_application(query)?;
            println!("selected {} (PID {})", application.name, application.pid);
            return Ok(CaptureConfig::for_pid(application.pid));
        }
        eprintln!("capturing system audio; use --app NAME or --pid PID to isolate an application");
        Ok(CaptureConfig::for_default_output())
    }
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum ModeArg {
    Online,
    Offline,
    #[default]
    TwoPass,
}

impl From<ModeArg> for AsrMode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::Online => Self::Online,
            ModeArg::Offline => Self::Offline,
            ModeArg::TwoPass => Self::TwoPass,
        }
    }
}

struct CtrlCSignal {
    #[cfg(windows)]
    listener: tokio::signal::windows::CtrlC,
}

impl CtrlCSignal {
    fn new() -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            Ok(Self {
                listener: tokio::signal::windows::ctrl_c()?,
            })
        }
        #[cfg(not(windows))]
        {
            Ok(Self {})
        }
    }

    async fn recv(&mut self) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            let _ = self.listener.recv().await;
            Ok(())
        }
        #[cfg(not(windows))]
        {
            tokio::signal::ctrl_c().await
        }
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Apps => print_audio_applications(&list_audio_applications()?),
        Command::Capture(args) => {
            tokio::task::spawn_blocking(move || capture_to_wav(args))
                .await
                .map_err(|error| Error::Native(format!("capture task failed: {error}")))??;
        }
        Command::Transcribe(args) => transcribe(args).await?,
    }
    Ok(())
}

fn capture_to_wav(args: CaptureArgs) -> Result<()> {
    let config = args.source.capture_config()?;
    let requested_process = matches!(config.source, CaptureSource::Process { .. });
    let mut session = CaptureSession::start(config)?;
    warn_if_process_fell_back(requested_process, &session);
    let deadline = Instant::now() + Duration::from_secs(args.seconds);
    let mut writer = None;
    let mut sample_frames = 0_u64;

    while Instant::now() < deadline {
        let timeout = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(250));
        let Some(frame) = receive_frame(&session, timeout)? else {
            continue;
        };
        let wav = match writer.as_mut() {
            Some(wav) => wav,
            None => writer.insert(WavWriter::create(&args.output, frame.format)?),
        };
        sample_frames += frame.frame_count() as u64;
        wav.write_frame(&frame)?;
    }

    session.check_health()?;
    session.stop()?;
    if let Some(writer) = writer {
        writer.finish()?;
    }
    println!(
        "captured {sample_frames} sample frames to {}",
        args.output.display()
    );
    Ok(())
}

async fn transcribe(args: TranscribeArgs) -> Result<()> {
    let capture_config = args.source.capture_config()?;
    let requested_process = matches!(capture_config.source, CaptureSource::Process { .. });
    let mut config = FunAsrConfig::new(&args.funasr_url);
    config.mode = args.mode.into();
    config.hotwords = args.hotwords.clone();
    config.audio_queue_capacity = args.queue_capacity;
    config.bearer_token = std::env::var(&args.bearer_token_env)
        .ok()
        .filter(|token| !token.is_empty());

    let mut client = FunAsrClient::connect(config).await?;
    let input = client.input();
    let events = client.take_events()?;
    let text_output = args.text_output.clone();
    let json_output = args.json_output.clone();
    let event_task = tokio::task::spawn_blocking(move || {
        write_transcript_events(events, text_output, json_output)
    });

    let seconds = args.seconds;
    let stop_requested = Arc::new(AtomicBool::new(false));
    let audio_stop_requested = Arc::clone(&stop_requested);
    // Keep the Windows listener alive through FunASR finalization. Tokio's
    // console handler delegates to the default terminating handler when no
    // Ctrl+C receivers remain.
    let mut ctrl_c = CtrlCSignal::new()?;
    let save_audio = args.save_audio.clone();
    let mut audio_task = tokio::task::spawn_blocking(move || -> Result<u64> {
        let mut session = CaptureSession::start(capture_config)?;
        if requested_process && session.mode() == CaptureMode::OutputLoopback {
            eprintln!(
                "warning: Windows 10 does not support PID-isolated capture; capturing the complete default output mix"
            );
        }
        let deadline = seconds.map(|seconds| Instant::now() + Duration::from_secs(seconds));
        let mut normalizer = None;
        let mut wav = None;
        let mut sent_chunks = 0_u64;

        while !audio_stop_requested.load(Ordering::Acquire)
            && deadline.is_none_or(|deadline| Instant::now() < deadline)
        {
            let timeout = deadline
                .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_millis(250))
                .min(Duration::from_millis(250));
            let Some(frame) = receive_frame(&session, timeout)? else {
                continue;
            };
            if let Some(path) = save_audio.as_ref()
                && wav.is_none()
            {
                wav = Some(WavWriter::create(path, frame.format)?);
            }
            if let Some(writer) = wav.as_mut() {
                writer.write_frame(&frame)?;
            }
            let converter = match normalizer.as_mut() {
                Some(converter) => converter,
                None => normalizer.insert(SpeechNormalizer::new(frame.format, 60)?),
            };
            for chunk in converter.push(&frame)? {
                input.try_send(chunk)?;
                sent_chunks += 1;
            }
        }

        if let Some(converter) = normalizer.as_mut() {
            for chunk in converter.finish()? {
                input.try_send(chunk)?;
                sent_chunks += 1;
            }
        }
        session.check_health()?;
        session.stop()?;
        if let Some(writer) = wav {
            writer.finish()?;
        }
        Ok(sent_chunks)
    });

    if seconds.is_none() {
        eprintln!("transcribing continuously; press Ctrl+C to stop and finalize");
    }
    let audio_join = tokio::select! {
        result = &mut audio_task => result,
        signal = ctrl_c.recv() => {
            if let Err(error) = signal {
                stop_requested.store(true, Ordering::Release);
                let _ = audio_task.await;
                return Err(Error::Io(error));
            }
            eprintln!("Ctrl+C received; stopping capture and waiting for the final FunASR result");
            stop_requested.store(true, Ordering::Release);
            audio_task.await
        }
    };
    let audio_result = audio_join
        .map_err(|error| Error::Native(format!("audio conversion task failed: {error}")))?;
    let finish_result = client.finish().await;
    let event_result = event_task
        .await
        .map_err(|error| Error::Native(format!("transcript writer task failed: {error}")))?;

    let sent_chunks = audio_result?;
    finish_result?;
    event_result?;
    println!("sent {sent_chunks} audio chunks to FunASR");
    Ok(())
}

fn warn_if_process_fell_back(requested_process: bool, session: &CaptureSession) {
    if requested_process && session.mode() == CaptureMode::OutputLoopback {
        eprintln!(
            "warning: Windows 10 does not support PID-isolated capture; capturing the complete default output mix"
        );
    }
}

fn receive_frame(
    session: &CaptureSession,
    timeout: Duration,
) -> Result<Option<vapor_tap::AudioFrame>> {
    match session.frames().recv_timeout(timeout) {
        Ok(frame) => {
            session.check_health()?;
            Ok(Some(frame))
        }
        Err(RecvTimeoutError::Timeout) => {
            session.check_health()?;
            Ok(None)
        }
        Err(RecvTimeoutError::Disconnected) => match session.check_health() {
            Err(error) => Err(error),
            Ok(()) => Err(Error::Native(
                "audio frame channel disconnected unexpectedly".into(),
            )),
        },
    }
}

fn print_audio_applications(applications: &[AudioApplication]) {
    if applications.is_empty() {
        println!("no applications are currently playing audio");
        return;
    }
    for (index, application) in applications.iter().enumerate() {
        let devices = if application.output_devices.is_empty() {
            String::new()
        } else {
            format!(" -> {}", application.output_devices.join(", "))
        };
        println!(
            "[{}] {} (PID {}){}",
            index + 1,
            application.name,
            application.pid,
            devices
        );
    }
}

fn write_transcript_events(
    mut events: FunAsrEventReceiver,
    text_path: Option<PathBuf>,
    json_path: Option<PathBuf>,
) -> Result<()> {
    let mut text = optional_writer(text_path)?;
    let mut jsonl = optional_writer(json_path)?;

    while let Some(event) = events.blocking_recv() {
        match &event {
            TranscriptEvent::Partial { text: partial } => {
                println!("[partial] {partial}");
            }
            TranscriptEvent::Final {
                text: final_text,
                timestamp,
            } => {
                println!("{final_text}");
                if let Some(writer) = text.as_mut() {
                    writeln!(writer, "{final_text}")?;
                    writer.flush()?;
                }
                if let Some(writer) = jsonl.as_mut() {
                    writeln!(
                        writer,
                        "{}",
                        json!({"type":"final", "text":final_text, "timestamp":timestamp})
                    )?;
                    writer.flush()?;
                }
            }
            TranscriptEvent::ServerError { message } => {
                eprintln!("FunASR server error: {message}");
            }
            TranscriptEvent::Disconnected { reason } => {
                eprintln!("FunASR disconnected: {reason}");
            }
            TranscriptEvent::End => break,
        }
        if let Some(writer) = jsonl.as_mut()
            && !matches!(event, TranscriptEvent::Final { .. })
        {
            let value = match event {
                TranscriptEvent::Partial { text } => json!({"type":"partial", "text":text}),
                TranscriptEvent::ServerError { message } => {
                    json!({"type":"server_error", "message":message})
                }
                TranscriptEvent::Disconnected { reason } => {
                    json!({"type":"disconnected", "reason":reason})
                }
                TranscriptEvent::End => json!({"type":"end"}),
                TranscriptEvent::Final { .. } => unreachable!(),
            };
            writeln!(writer, "{value}")?;
            writer.flush()?;
        }
    }
    Ok(())
}

fn optional_writer(path: Option<PathBuf>) -> Result<Option<BufWriter<File>>> {
    path.map(File::create)
        .transpose()
        .map(|file| file.map(BufWriter::new))
        .map_err(Error::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcribe_runs_until_ctrl_c_when_seconds_are_omitted() {
        let cli = Cli::try_parse_from([
            "vapor-tap",
            "transcribe",
            "--funasr-url",
            "ws://127.0.0.1:10095",
        ])
        .unwrap();

        let Command::Transcribe(args) = cli.command else {
            panic!("expected transcribe command");
        };
        assert_eq!(args.seconds, None);
    }

    #[test]
    fn transcribe_accepts_an_optional_fixed_duration() {
        let cli = Cli::try_parse_from([
            "vapor-tap",
            "transcribe",
            "--seconds",
            "60",
            "--funasr-url",
            "ws://127.0.0.1:10095",
        ])
        .unwrap();

        let Command::Transcribe(args) = cli.command else {
            panic!("expected transcribe command");
        };
        assert_eq!(args.seconds, Some(60));
    }

    #[test]
    fn omitted_source_defaults_to_system_audio() {
        let cli = Cli::try_parse_from(["vapor-tap", "capture"]).unwrap();
        let Command::Capture(args) = cli.command else {
            panic!("expected capture command");
        };

        assert_eq!(
            args.source.capture_config().unwrap().source,
            CaptureSource::OutputDevice { name: None }
        );
    }

    #[test]
    fn named_output_device_is_not_a_cli_option() {
        assert!(Cli::try_parse_from(["vapor-tap", "capture", "--device", "Speakers"]).is_err());
    }
}
