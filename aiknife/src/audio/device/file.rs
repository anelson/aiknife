//! Implementation of the audio device abstraction for reading audio from a file.
use crate::{
    audio::{self, device},
    util::threading,
};
use anyhow::{Context, Result};
use std::{path::PathBuf, sync::Arc, time::Duration};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{DecoderOptions, CODEC_TYPE_NULL},
    formats::FormatOptions,
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
};
use tracing::*;

pub(crate) struct FileAudioInput {
    file_path: PathBuf,
    device_name: String,
}

impl FileAudioInput {
    pub fn new(file_path: impl Into<PathBuf>) -> Result<Self> {
        let file_path = file_path.into();
        let device_name = format!("file:{}", file_path.display());

        Ok(Self {
            file_path,
            device_name,
        })
    }
}

impl device::AudioInput for FileAudioInput {
    fn device_name(&self) -> &str {
        &self.device_name
    }

    #[instrument(skip_all, fields(path = %self.file_path.display()))]
    fn start_stream(
        &mut self,
        config: audio::AudioInputConfig,
    ) -> Result<(
        Box<dyn audio::AudioStreamGuard>,
        Box<dyn audio::AudioInputStream>,
    )> {
        let config = Arc::new(config);

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
        let track_id = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .context("no audio track found")?
            .id;

        let decoder = symphonia::default::get_codecs()
            .make(
                &format
                    .tracks()
                    .iter()
                    .find(|t| t.id == track_id)
                    .unwrap()
                    .codec_params,
                &decoder_opts,
            )
            .context("unsupported audio codec")?;

        let device_name = self.device_name.clone();
        let sample_rate = format
            .tracks()
            .iter()
            .find(|t| t.id == track_id)
            .unwrap()
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

        if let Some(n_frames) = format
            .tracks()
            .iter()
            .find(|t| t.id == track_id)
            .unwrap()
            .codec_params
            .n_frames
        {
            debug!("Audio file contains {} frames", n_frames);
        }

        // TODO: Is 32 a reasonable bound here?  Unlike when streaming from an audio input device,
        // it's almost certain that we can read audio from this file at many hundreds of times faster
        // than realtime, so depending upon how many samples are in each symphonia read this channel
        // should fill up quickly.
        let chunk_channel_bound = 32;

        let (stop_signal_sender, running_receiver, final_receiver) =
            threading::start_func_as_thread_worker(
                "audio_file_stream_host_thread",
                chunk_channel_bound,
                move |stop_signal_receiver, chunk_sender| {
                    let mut decoder = decoder;
                    let mut format = format;
                    let mut timestamp = Duration::default();

                    loop {
                        // Check if we should abort
                        if stop_signal_receiver.is_stop_signaled() {
                            debug!("Received abort signal; stopping audio stream");
                            break;
                        }

                        // Get the next packet from the format reader
                        // Surprisingly, in normal, correct operation of symphonia, when a stream
                        // ends the next_packet call returns an EOF error.  This is, apparently,
                        // normal.
                        let packet = match format.next_packet() {
                            Err(symphonia::core::errors::Error::IoError(e))
                                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                            {
                                // TODO: Is this really not a reportable error condition?  Does any
                                // read of any legitimate format end with this error?
                                debug!("Reached end of file");
                                break;
                            }
                            Err(e) => {
                                error!(%e, "Error reading audio file");
                                return Err(e.into());
                            }
                            Ok(packet) => packet,
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
                        let decoded = decoder
                            .decode(&packet)
                            .context("Error decoding audio packet")?;

                        // Create an audio buffer and copy the decoded audio into it
                        let spec = *decoded.spec();
                        let mut sample_buf =
                            SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
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

                            let chunk = audio::AudioInputChunk {
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

                    Result::<()>::Ok(())
                },
            );

        let stream = audio::AudioInputThreadingChunksReceiver::new(
            device_name,
            std::num::NonZeroU32::new(sample_rate).unwrap(),
            threading::CombinedOutputReceiver::join(running_receiver, final_receiver),
        );

        Ok((Box::new(Some(stop_signal_sender)), Box::new(stream)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio::AudioInput;

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

        let mut device = FileAudioInput::new(&test_file)?;

        let config = audio::AudioInputConfig::default();

        let (_guard, stream) = device.start_stream(config)?;

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
        let mut device = FileAudioInput::new(&test_file)?;
        let config = audio::AudioInputConfig::default();

        let (_guard, stream) = device.start_stream(config.clone())?;
        let mut total_samples = 0;
        for result in stream {
            let chunk = result?;
            println!("Got chunk with {} samples", chunk.samples.len());
            total_samples += chunk.samples.len();
        }
        println!("Long sample contains {total_samples} samples");

        // Now read again but stop after first chunk
        let mut device = FileAudioInput::new(&test_file)?;
        let (mut guard, mut stream) = device.start_stream(config)?;

        // Get first chunk
        let first_chunk = stream.next().unwrap()?;
        let first_chunk_samples = first_chunk.samples.len();

        // stop the stream
        guard.stop_stream();

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

    #[test]
    fn test_file_audio_device_decoding() -> Result<()> {
        use symphonia::core::codecs::DecoderOptions;
        use symphonia::core::formats::FormatOptions;
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;

        // Test both our test files
        for test_file in [test_short_audio_file_path(), test_long_audio_file_path()] {
            debug!("Testing file: {}", test_file.display());

            // First read using our FileAudioDevice
            let mut device = FileAudioInput::new(&test_file)?;
            let config = audio::AudioInputConfig::default();
            let (_guard, stream) = device.start_stream(config)?;

            // Collect all samples from our device
            let mut our_samples = Vec::new();
            for result in stream {
                let chunk = result?;
                our_samples.extend(chunk.samples);
            }

            // Now read using symphonia directly
            let file = std::fs::File::open(&test_file)?;
            let mss = MediaSourceStream::new(Box::new(file), Default::default());
            let mut hint = Hint::new();
            if let Some(extension) = test_file.extension().and_then(|e| e.to_str()) {
                hint.with_extension(extension);
            }

            let format_opts = FormatOptions::default();
            let metadata_opts = MetadataOptions::default();
            let decoder_opts = DecoderOptions::default();

            let probed = symphonia::default::get_probe()
                .format(&hint, mss, &format_opts, &metadata_opts)
                .context("unsupported audio format")?;

            let mut format = probed.format;
            let track_id = format
                .tracks()
                .iter()
                .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
                .context("no audio track found")?
                .id;

            let mut decoder = symphonia::default::get_codecs()
                .make(
                    &format
                        .tracks()
                        .iter()
                        .find(|t| t.id == track_id)
                        .unwrap()
                        .codec_params,
                    &decoder_opts,
                )
                .context("unsupported audio codec")?;

            // Read all samples from symphonia directly
            let mut reference_samples = Vec::new();
            while let Ok(packet) = format.next_packet() {
                if packet.track_id() != track_id {
                    continue;
                }

                let decoded = decoder.decode(&packet)?;
                let spec = *decoded.spec();
                let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                sample_buf.copy_interleaved_ref(decoded);

                // Average channels the same way our device does
                if spec.channels.count() > 1 {
                    let channel_count = spec.channels.count();
                    let samples = sample_buf.samples();
                    reference_samples.extend(
                        samples
                            .chunks(channel_count)
                            .map(|chunk| chunk.iter().sum::<f32>() / channel_count as f32),
                    );
                } else {
                    reference_samples.extend(sample_buf.samples());
                }
            }

            // Verify we got roughly the same number of samples
            // Note: There might be small differences due to decoder padding/alignment
            let sample_diff = (our_samples.len() as i64 - reference_samples.len() as i64).abs();
            assert!(
                sample_diff < 100,
                "Sample count mismatch for {}: ours={}, reference={}, diff={}",
                test_file.display(),
                our_samples.len(),
                reference_samples.len(),
                sample_diff
            );

            // Compare sample values
            // Note: We use a relatively large epsilon because:
            // 1. Different decoders might use slightly different algorithms
            // 2. Our channel averaging might introduce small differences
            // 3. We're more interested in catching major decoding issues
            let max_diff = our_samples
                .iter()
                .zip(reference_samples.iter())
                .map(|(a, b)| (a - b).abs())
                .max_by(|a, b| a.partial_cmp(b).unwrap())
                .unwrap_or(0.0);

            assert!(
                max_diff < 0.1,
                "Sample value mismatch for {}: max difference = {}",
                test_file.display(),
                max_diff
            );

            // Verify we have some non-zero samples (audio isn't silent)
            let has_non_zero = our_samples.iter().any(|&s| s.abs() > 0.0001);
            assert!(
                has_non_zero,
                "Audio appears to be completely silent: {}",
                test_file.display()
            );
        }

        Ok(())
    }
}
