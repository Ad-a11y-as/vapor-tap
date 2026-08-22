use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use vapor_tap::{CaptureConfig, CaptureSession, Result, WavWriter};

#[derive(Parser, Debug)]
#[command(version, about = "Capture audio produced by one process")]
struct Args {
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

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
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
