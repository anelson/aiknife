//! Implementation of the audio device abstraction for reading audio from a file.
use super::{AudioInput, AudioInputChunk, AudioInputStream, AudioStreamGuard};
use crate::audio::{AudioInputConfig, AudioInputCrossbeamChannelReceiver};
use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{DecoderOptions, CODEC_TYPE_NULL},
    formats::FormatOptions,
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
};
use tracing::*;

pub(crate) struct FileAudioDevice {
    file_path: PathBuf,
    device_name: String,
}

impl FileAudioDevice {
    pub(crate) fn new(file_path: impl Into<PathBuf>) -> Result<Self> {
        let file_path = file_path.into();
        let device_name = format!("file:{}", file_path.display());

        Ok(Self {
            file_path,
            device_name,
        })
    }
}

impl AudioInput for FileAudioDevice {
    fn device_name(&self) -> &str {
        &self.device_name
    }

    #[instrument(skip_all, fields(path = %self.file_path.display()))]
    fn start_stream(
        &mut self,
        config: Arc<AudioInputConfig>,
    ) -> Result<(Box<dyn AudioStreamGuard>, Box<dyn AudioInputStream>)> {
        // Open the media source
        let file = std::fs::File::open(&self.file_path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        // Create a hint to help the format registry guess what format reader is appropriate
        let mut hint = Hint::new();
        if let Some(extension) = self.file_path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(extension);
        }

        // Use the default options for format reader
        let format_opts = FormatOptions::default();
        let metadata_opts = MetadataOptions::default();
        let decoder_opts = DecoderOptions::default();

        // Probe the media source to determine the format
        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &format_opts, &metadata_opts)
            .context("unsupported audio format")?;

        let format = probed.format;

        // Find all audio tracks
        let audio_tracks: Vec<_> = format
            .tracks()
            .iter()
            .filter(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .collect();

        // Verify we have exactly one audio track
        let track = match audio_tracks.len() {
            0 => anyhow::bail!("no audio tracks found in file"),
            1 => audio_tracks[0],
            n => anyhow::bail!("file contains {n} audio tracks - only files with a single audio track are supported"),
        };

        let track_id = track.id;

        // Create a decoder for the track
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &decoder_opts)
            .context("unsupported audio codec")?;

        let device_name = self.device_name.clone();
        let sample_rate = track
            .codec_params
            .sample_rate
            .ok_or_else(|| anyhow::anyhow!("unknown sample rate"))?;

        // If input_sample_rate is specified, verify it matches
        if let Some(requested_rate) = config.input_sample_rate {
            if requested_rate.get() != sample_rate {
                anyhow::bail!(
                    "requested sample rate {} does not match audio file sample rate {}",
                    requested_rate,
                    sample_rate
                );
            }
        }

        // Create channels for communication between the decoder thread and the stream
        // TODO: Is 32 a reasonable bound here?  Unlike when streaming from an audio input device,
        // it's almost certain that we can read audio from this file at many hundreds of times faster
        // than realtime, so depending upon how many samples are in each symphonia read this channel
        // should fill up quickly.
        let (chunk_sender, chunk_receiver) = crossbeam::channel::bounded(32);
        let (abort_sender, abort_receiver) = crossbeam::channel::bounded(0);

        let current_span = Span::current();

        // Spawn a thread to decode the audio file
        std::thread::spawn(move || {
            let span = debug_span!(parent: &current_span, "audio_decode_thread");
            let _guard = span.enter();
            let mut decoder = decoder;
            let mut format = format;
            let mut timestamp = Duration::default();

            loop {
                // Check if we should abort
                match abort_receiver.try_recv() {
                    Ok(()) => {
                        debug!("Received abort signal; stopping audio stream");
                        break;
                    }
                    Err(crossbeam::channel::TryRecvError::Disconnected) => {
                        debug!("Error receiving abort signal; assuming that the stream guard struct was dropped and that this constitutes an abort signal");
                        break;
                    }
                    Err(crossbeam::channel::TryRecvError::Empty) => {
                        // No abort signal has been sent so keep processing
                    }
                }

                // Get the next packet from the format reader
                let packet = match format.next_packet() {
                    Ok(packet) => packet,
                    Err(symphonia::core::errors::Error::IoError(e)) => {
                        if e.kind() == std::io::ErrorKind::UnexpectedEof {
                            // TODO: Is this really not a reportable error condition?  Does any
                            // read of any legitimate format end with this error?
                            debug!("Reached end of file");
                            break;
                        }
                        error!(%e, "IO error reading audio file");
                        let _ = chunk_sender.send(Err(e.into()));
                        break;
                    }
                    Err(e) => {
                        error!(%e, "Error reading audio file");
                        let _ = chunk_sender.send(Err(e.into()));
                        break;
                    }
                };

                // Skip packets from other tracks
                if packet.track_id() != track_id {
                    continue;
                }

                trace!(
                    ts = packet.ts(),
                    dur = packet.dur(),
                    bytes = packet.data.len(),
                    "Decoded individual audio packet"
                );

                // Decode the packet
                let decoded = match decoder.decode(&packet) {
                    Ok(decoded) => decoded,
                    Err(e) => {
                        error!(%e, "Error decoding audio packet");
                        let _ = chunk_sender.send(Err(e.into()));
                        break;
                    }
                };

                // Create an audio buffer and copy the decoded audio into it
                let spec = *decoded.spec();
                let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                sample_buf.copy_interleaved_ref(decoded);

                // Convert to mono if needed by averaging all channels
                // BUG: I'm sure AI generated this wrong.  Samples from each channel are
                // interleaved, averaging this way will make the audio nonsensical.
                // Actually according to the symphonia sample, the samples might be interleaved or
                // they might be in "planar channel order", depending upon the format.
                let samples = if spec.channels.count() > 1 {
                    let channel_count = spec.channels.count();
                    let samples = sample_buf.samples();
                    samples
                        .chunks(channel_count)
                        .map(|chunk| chunk.iter().sum::<f32>() / channel_count as f32)
                        .collect()
                } else {
                    sample_buf.samples().to_vec()
                };

                if !samples.is_empty() {
                    let duration =
                        Duration::from_secs_f64(samples.len() as f64 / sample_rate as f64);

                    let chunk = AudioInputChunk {
                        timestamp,
                        duration,
                        samples,
                    };

                    timestamp += duration;

                    if chunk_sender.send(Ok(chunk)).is_err() {
                        debug!("Receiver dropped; stopping decode thread");
                        break;
                    }
                }
            }

            debug!("Audio decode thread exiting");
        });

        let stream = AudioInputCrossbeamChannelReceiver::new(
            device_name,
            std::num::NonZeroU32::new(sample_rate).unwrap(),
            chunk_receiver,
        );

        Ok((
            Box::new(SymphoniaStreamGuard(Arc::new(abort_sender))),
            Box::new(stream),
        ))
    }
}

#[derive(Clone, Debug)]
struct SymphoniaStreamGuard(Arc<crossbeam::channel::Sender<()>>);

impl Drop for SymphoniaStreamGuard {
    fn drop(&mut self) {
        <Self as AudioStreamGuard>::stop_stream(self);
    }
}

impl AudioStreamGuard for SymphoniaStreamGuard {
    fn stop_stream(&mut self) {
        let _ = self.0.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU32;

    /// Return the local path to a test short 10 second audio file that can be used for testing.
    fn test_short_audio_file_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test_data")
            .join("audio")
            .join("samples_jfk.wav")
    }

    /// Return the local path to a test long 8 minute audio file that can be used for testing.
    fn test_long_audio_file_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test_data")
            .join("audio")
            .join("samples_fdr_pearl_harbor.ogg")
    }

    #[test]
    fn test_file_audio_device() -> Result<()> {
        crate::test_helpers::init_test_logging();

        let test_file = test_short_audio_file_path();

        let mut device = FileAudioDevice::new(&test_file)?;

        let config = Arc::new(AudioInputConfig::default());

        let (_guard, mut stream) = device.start_stream(config)?;

        // Collect all audio
        let mut samples = Vec::new();
        for result in stream {
            let chunk = result?;
            samples.extend(chunk.samples);
        }

        // Verify we got some samples
        assert!(!samples.is_empty(), "Should have received audio samples");

        // Check that the audio isn't completely silent
        let has_non_zero = samples.iter().any(|&s| s.abs() > 0.0001);
        assert!(has_non_zero, "Audio appears to be completely silent");

        Ok(())
    }

    #[test]
    fn test_file_audio_device_early_stop() -> Result<()> {
        crate::test_helpers::init_test_logging();

        // For this test we need to use a long audio file, otherwise we can read it so fast that
        // it's already fully loaded into memory and put in the channel before we can stop the
        // stream.
        let test_file = test_long_audio_file_path();

        // First get the total number of samples by reading the whole file
        let mut device = FileAudioDevice::new(&test_file)?;
        let config = Arc::new(AudioInputConfig::default());

        let (_guard, mut stream) = device.start_stream(config.clone())?;
        let mut total_samples = 0;
        for result in stream {
            total_samples += result?.samples.len();
        }

        // Now read again but stop after first chunk
        let mut device = FileAudioDevice::new(&test_file)?;
        let (guard, mut stream) = device.start_stream(config)?;

        // Get first chunk
        let first_chunk = stream.next().unwrap()?;
        let first_chunk_samples = first_chunk.samples.len();

        // Drop the guard to stop the stream
        drop(guard);

        // Collect remaining buffered samples
        let mut remaining_samples = 0;
        for result in stream {
            remaining_samples += result?.samples.len();
        }

        let collected_samples = first_chunk_samples + remaining_samples;
        assert!(
            collected_samples < total_samples,
            "Expected fewer samples ({collected_samples}) than total samples ({total_samples}) due to early stop"
        );

        Ok(())
    }
}
