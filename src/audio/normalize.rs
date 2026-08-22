use std::time::Duration;

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
    calculate_cutoff,
};

use crate::{AudioFormat, AudioFrame, Error, Result};

const OUTPUT_RATE: u32 = 16_000;
const RESAMPLER_INPUT_FRAMES: usize = 1_024;

/// One chunk of mono, 16 kHz, signed 16-bit little-endian PCM.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pcm16Chunk {
    pub bytes: Vec<u8>,
    pub samples: usize,
}

impl Pcm16Chunk {
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.samples as f64 / OUTPUT_RATE as f64)
    }
}

/// Stateful conversion from native capture frames to FunASR-compatible PCM.
pub struct SpeechNormalizer {
    input_format: AudioFormat,
    resampler: Option<SincFixedIn<f32>>,
    mono_input: Vec<f32>,
    mono_offset: usize,
    output: Vec<f32>,
    output_offset: usize,
    output_chunk_samples: usize,
    finished: bool,
}

impl SpeechNormalizer {
    /// Creates a converter. `chunk_duration_ms` normally uses 60 ms for
    /// low-latency WebSocket delivery.
    pub fn new(input_format: AudioFormat, chunk_duration_ms: u32) -> Result<Self> {
        if input_format.sample_rate == 0 || input_format.channels == 0 {
            return Err(Error::InvalidArgument("invalid input audio format"));
        }
        if chunk_duration_ms == 0 {
            return Err(Error::InvalidArgument("chunk duration must be non-zero"));
        }
        let output_chunk_samples =
            (u64::from(OUTPUT_RATE) * u64::from(chunk_duration_ms) / 1_000) as usize;
        if output_chunk_samples == 0 {
            return Err(Error::InvalidArgument("chunk duration is too short"));
        }

        let resampler = if input_format.sample_rate == OUTPUT_RATE {
            None
        } else {
            let sinc_len = 128;
            let window = WindowFunction::Blackman2;
            let parameters = SincInterpolationParameters {
                sinc_len,
                f_cutoff: calculate_cutoff(sinc_len, window),
                oversampling_factor: 128,
                interpolation: SincInterpolationType::Cubic,
                window,
            };
            Some(
                SincFixedIn::<f32>::new(
                    OUTPUT_RATE as f64 / input_format.sample_rate as f64,
                    1.0,
                    parameters,
                    RESAMPLER_INPUT_FRAMES,
                    1,
                )
                .map_err(|error| Error::AudioConversion(error.to_string()))?,
            )
        };

        Ok(Self {
            input_format,
            resampler,
            mono_input: Vec::with_capacity(RESAMPLER_INPUT_FRAMES * 2),
            mono_offset: 0,
            output: Vec::with_capacity(output_chunk_samples * 2),
            output_offset: 0,
            output_chunk_samples,
            finished: false,
        })
    }

    pub const fn output_format() -> AudioFormat {
        AudioFormat {
            sample_rate: OUTPUT_RATE,
            channels: 1,
        }
    }

    /// Adds a native capture packet and returns every complete PCM chunk now
    /// available. The converter keeps filter and partial-chunk state.
    pub fn push(&mut self, frame: &AudioFrame) -> Result<Vec<Pcm16Chunk>> {
        if self.finished {
            return Err(Error::InvalidArgument("normalizer is already finished"));
        }
        if frame.format != self.input_format {
            return Err(Error::InvalidArgument(
                "audio format changed during transcription",
            ));
        }
        let channels = usize::from(self.input_format.channels);
        if frame.samples.len() % channels != 0 {
            return Err(Error::InvalidArgument(
                "audio packet is not aligned to its channel count",
            ));
        }

        for interleaved_frame in frame.samples.chunks_exact(channels) {
            let sum: f32 = interleaved_frame.iter().copied().sum();
            self.mono_input.push(sum / channels as f32);
        }
        self.process_complete_resampler_chunks()?;
        Ok(self.take_complete_output_chunks())
    }

    /// Flushes the resampler and returns the final zero-padded PCM chunk.
    pub fn finish(&mut self) -> Result<Vec<Pcm16Chunk>> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;

        if let Some(resampler) = self.resampler.as_mut() {
            let remaining = &self.mono_input[self.mono_offset..];
            if !remaining.is_empty() {
                let input = [remaining];
                let resampled = resampler
                    .process_partial(Some(&input), None)
                    .map_err(|error| Error::AudioConversion(error.to_string()))?;
                self.output.extend_from_slice(&resampled[0]);
            }
        } else {
            self.output
                .extend_from_slice(&self.mono_input[self.mono_offset..]);
        }
        self.mono_input.clear();
        self.mono_offset = 0;

        let mut chunks = self.take_complete_output_chunks();
        let remaining = &self.output[self.output_offset..];
        if !remaining.is_empty() {
            let mut padded = vec![0.0; self.output_chunk_samples];
            padded[..remaining.len()].copy_from_slice(remaining);
            chunks.push(pcm_chunk(&padded));
            self.output_offset = self.output.len();
        }
        self.compact_buffers();
        Ok(chunks)
    }

    fn process_complete_resampler_chunks(&mut self) -> Result<()> {
        let Some(resampler) = self.resampler.as_mut() else {
            self.output
                .extend_from_slice(&self.mono_input[self.mono_offset..]);
            self.mono_offset = self.mono_input.len();
            self.compact_buffers();
            return Ok(());
        };

        while self.mono_input.len() - self.mono_offset >= RESAMPLER_INPUT_FRAMES {
            let end = self.mono_offset + RESAMPLER_INPUT_FRAMES;
            let input = [&self.mono_input[self.mono_offset..end]];
            let resampled = resampler
                .process(&input, None)
                .map_err(|error| Error::AudioConversion(error.to_string()))?;
            self.output.extend_from_slice(&resampled[0]);
            self.mono_offset = end;
        }
        self.compact_buffers();
        Ok(())
    }

    fn take_complete_output_chunks(&mut self) -> Vec<Pcm16Chunk> {
        let mut chunks = Vec::new();
        while self.output.len() - self.output_offset >= self.output_chunk_samples {
            let end = self.output_offset + self.output_chunk_samples;
            chunks.push(pcm_chunk(&self.output[self.output_offset..end]));
            self.output_offset = end;
        }
        self.compact_buffers();
        chunks
    }

    fn compact_buffers(&mut self) {
        if self.mono_offset >= RESAMPLER_INPUT_FRAMES * 4 {
            self.mono_input.drain(..self.mono_offset);
            self.mono_offset = 0;
        }
        if self.output_offset >= self.output_chunk_samples * 4 {
            self.output.drain(..self.output_offset);
            self.output_offset = 0;
        }
    }
}

fn pcm_chunk(samples: &[f32]) -> Pcm16Chunk {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        let scaled = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round();
        let value = if scaled <= i16::MIN as f32 {
            i16::MIN
        } else if scaled >= i16::MAX as f32 {
            i16::MAX
        } else {
            scaled as i16
        };
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    Pcm16Chunk {
        bytes,
        samples: samples.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(rate: u32, channels: u16) -> AudioFormat {
        AudioFormat {
            sample_rate: rate,
            channels,
        }
    }

    #[test]
    fn downmixes_and_chunks_16k_stereo() {
        let mut normalizer = SpeechNormalizer::new(format(16_000, 2), 60).unwrap();
        let mut samples = Vec::new();
        for _ in 0..960 {
            samples.extend_from_slice(&[0.5, -0.5]);
        }
        let chunks = normalizer
            .push(&AudioFrame {
                format: format(16_000, 2),
                samples,
            })
            .unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].samples, 960);
        assert!(chunks[0].bytes.iter().all(|byte| *byte == 0));
        assert_eq!(chunks[0].duration(), Duration::from_millis(60));
    }

    #[test]
    fn resamples_48k_and_flushes_a_final_chunk() {
        let mut normalizer = SpeechNormalizer::new(format(48_000, 1), 60).unwrap();
        let frame = AudioFrame {
            format: format(48_000, 1),
            samples: vec![0.25; 4_800],
        };
        let mut chunks = normalizer.push(&frame).unwrap();
        chunks.extend(normalizer.finish().unwrap());
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|chunk| chunk.bytes.len() == 1_920));
    }

    #[test]
    fn rejects_format_changes() {
        let mut normalizer = SpeechNormalizer::new(format(48_000, 2), 60).unwrap();
        let error = normalizer
            .push(&AudioFrame {
                format: format(44_100, 2),
                samples: vec![0.0; 200],
            })
            .unwrap_err();
        assert!(matches!(error, Error::InvalidArgument(_)));
    }
}
