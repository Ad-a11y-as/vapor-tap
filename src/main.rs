use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::json;
use vapor_tap::asr::{AsrMode, FunAsrClient, FunAsrConfig, FunAsrEventReceiver, TranscriptEvent};
use vapor_tap::audio::SpeechNormalizer;
use vapor_tap::{CaptureConfig, CaptureSession, Error, Result, WavWriter};

#[derive(Parser, Debug)]
#[command(version, about = "Capture or transcribe audio produced by one process")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Capture process audio to an IEEE-float WAV file.
    Capture(CaptureArgs),
    /// Stream process audio to a remote FunASR WebSocket service.
    Transcribe(TranscribeArgs),
}

#[derive(Args, Debug)]
struct CaptureArgs {
    /// Target process ID. On Windows its child-process tree is included.
    #[arg(long)]
    pid: u32,
    /// Capture duration in seconds.
    #[arg(long, default_value_t = 10)]
    seconds: u64,
    /// Destination IEEE-float WAV file.
    #[arg(long, default_value = "capture.wav")]
    output: PathBuf,
}

#[derive(Args, Debug)]
struct TranscribeArgs {
    /// Target process ID. On Windows its child-process tree is included.
    #[arg(long)]
    pid: u32,
    /// Capture duration in seconds.
    #[arg(long, default_value_t = 60)]
    seconds: u64,
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

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    match Cli::parse().command {
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
    let mut session = CaptureSession::start(CaptureConfig::for_pid(args.pid))?;
    let deadline = Instant::now() + Duration::from_secs(args.seconds);
    let mut writer = None;
    let mut sample_frames = 0_u64;

    while Instant::now() < deadline {
        let timeout = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(250));
        let Ok(frame) = session.frames().recv_timeout(timeout) else {
            continue;
        };
        let wav = match writer.as_mut() {
            Some(wav) => wav,
            None => writer.insert(WavWriter::create(&args.output, frame.format)?),
        };
        sample_frames += frame.frame_count() as u64;
        wav.write_frame(&frame)?;
    }

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

    let pid = args.pid;
    let seconds = args.seconds;
    let save_audio = args.save_audio.clone();
    let audio_task = tokio::task::spawn_blocking(move || -> Result<u64> {
        let mut session = CaptureSession::start(CaptureConfig::for_pid(pid))?;
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut normalizer = None;
        let mut wav = None;
        let mut sent_chunks = 0_u64;

        while Instant::now() < deadline {
            let timeout = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(250));
            let Ok(frame) = session.frames().recv_timeout(timeout) else {
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
        session.stop()?;
        if let Some(writer) = wav {
            writer.finish()?;
        }
        Ok(sent_chunks)
    });

    let audio_result = audio_task
        .await
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
