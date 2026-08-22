use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use crate::{AudioFormat, AudioFrame, Error, Result};

/// Streaming IEEE-float WAV writer. The header is finalized on `finish` or drop.
pub struct WavWriter {
    file: Option<BufWriter<File>>,
    format: AudioFormat,
    data_bytes: u32,
}

impl WavWriter {
    pub fn create(path: impl AsRef<Path>, format: AudioFormat) -> Result<Self> {
        if format.channels == 0 || format.sample_rate == 0 {
            return Err(Error::InvalidArgument("invalid WAV format"));
        }
        let mut file = BufWriter::new(File::create(path)?);
        file.write_all(&[0; 44])?;
        Ok(Self {
            file: Some(file),
            format,
            data_bytes: 0,
        })
    }

    pub fn write_frame(&mut self, frame: &AudioFrame) -> Result<()> {
        if frame.format != self.format {
            return Err(Error::InvalidArgument(
                "audio format changed during WAV output",
            ));
        }
        let bytes = frame
            .samples
            .len()
            .checked_mul(4)
            .ok_or(Error::InvalidArgument("audio packet is too large for WAV"))?;
        let new_size = self
            .data_bytes
            .checked_add(
                u32::try_from(bytes).map_err(|_| Error::InvalidArgument("WAV exceeds 4 GiB"))?,
            )
            .ok_or(Error::InvalidArgument("WAV exceeds 4 GiB"))?;
        let file = self.file.as_mut().expect("writer already finished");
        for sample in &frame.samples {
            file.write_all(&sample.to_le_bytes())?;
        }
        self.data_bytes = new_size;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.finalize()
    }

    fn finalize(&mut self) -> Result<()> {
        let Some(mut file) = self.file.take() else {
            return Ok(());
        };
        let block_align = self.format.channels * 4;
        let byte_rate = self.format.sample_rate * u32::from(block_align);
        let mut header = Vec::with_capacity(44);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(36 + self.data_bytes).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16_u32.to_le_bytes());
        header.extend_from_slice(&3_u16.to_le_bytes()); // WAVE_FORMAT_IEEE_FLOAT
        header.extend_from_slice(&self.format.channels.to_le_bytes());
        header.extend_from_slice(&self.format.sample_rate.to_le_bytes());
        header.extend_from_slice(&byte_rate.to_le_bytes());
        header.extend_from_slice(&block_align.to_le_bytes());
        header.extend_from_slice(&32_u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&self.data_bytes.to_le_bytes());
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header)?;
        file.flush()?;
        Ok(())
    }
}

impl Drop for WavWriter {
    fn drop(&mut self) {
        let _ = self.finalize();
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn writes_float_wav_header_and_samples() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("audio.wav");
        let format = AudioFormat {
            sample_rate: 48_000,
            channels: 2,
        };
        let mut writer = WavWriter::create(&path, format).unwrap();
        writer
            .write_frame(&AudioFrame {
                format,
                samples: vec![0.25, -0.25, 0.5, -0.5],
            })
            .unwrap();
        writer.finish().unwrap();

        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 16);
        assert_eq!(bytes.len(), 60);
    }
}
