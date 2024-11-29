//! System audio device management functions, including convenient functions for discovering all audio
//! devices in a way that's suitable for listing them to users, and picking a reasonable default.
//!
//! TODO: Handle changes to available system audio devices on the fly (ie, AirPods connecting to a
//! Mac).
use crate::{
    audio::{self as audio, stream, AudioInputChunk},
    util::threading,
};
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use either::Either;
use itertools::Itertools;
use std::time::Duration;
use std::{num::NonZeroU32, sync::Arc};
use tracing::*;

/// List the names of all of the available audio devices on the system that are suitable for
/// use.
///
/// Returns a tuple; the first is a vector of input device names, the second is a vector of output
/// device names.
pub fn list_device_names() -> Result<(Vec<String>, Vec<String>)> {
    let (input, output) = list_devices()?;

    Ok((
        devices_iter_to_device_names(input.iter().map(|d| &d.device)),
        devices_iter_to_device_names(output.iter().map(|d| &d.device)),
    ))
}

fn devices_iter_to_device_names<'a>(devices: impl Iterator<Item = &'a AudioDevice>) -> Vec<String> {
    devices.map(|d| d.device_name.clone()).collect()
}

/// List all devices on the system that appear to be working either for input or output.
///
/// Any errors describing a particular device are logged but will not fail this function.  Devices
/// with errors are excluded from the list of discovered devices.
fn list_devices() -> Result<(Vec<AudioInputDevice>, Vec<AudioOutputDevice>)> {
    let mut devices = Vec::new();

    for host_id in cpal::available_hosts() {
        match cpal::host_from_id(host_id) {
            Ok(host) => match list_host_devices(host) {
                Ok(host_devices) => {
                    devices.extend(host_devices);
                }
                Err(e) => {
                    error!(host_id = host_id.name(), %e, "Error listing devices for host; all devices on this host are excluded");
                }
            },
            Err(e) => {
                error!(host_id = host_id.name(), %e, "Host appears to be unavailable");
            }
        }
    }

    // Split the devices into input and output devices
    let (input_devices, output_devices): (Vec<_>, Vec<_>) = devices
        .into_iter()
        .flat_map(|d: AudioDevice| match d.try_into_input_device() {
            Ok(d) => Some(Either::Left(d)),
            Err(d) => match d.try_into_output_device() {
                Ok(d) => Some(Either::Right(d)),
                Err(d) => {
                    debug!(device_name = %d.device_name,
                        device_index = d.device_index,
                        "Device does not support input or output");
                    None
                }
            },
        })
        .inspect(|either| match either {
            Either::Left(d) => {
                debug!(device_name = %d.device.device_name,
                        device_index = d.device.device_index,
                        "Found input device")
            }
            Either::Right(d) => {
                debug!(device_name = %d.device.device_name,
                        device_index = d.device.device_index,
                        "Found output device")
            }
        })
        .partition_map(|either: Either<AudioInputDevice, AudioOutputDevice>| either);

    Ok((input_devices, output_devices))
}

/// List the suitable input devices for a specific host.
fn list_host_devices(host: cpal::Host) -> Result<Vec<AudioDevice>> {
    Ok(host
        .devices()?
        .enumerate()
        .map(move |(device_index, device)| AudioDevice {
            host_id: host.id(),
            device_index,
            device_name: device.name().unwrap_or_default(),
            inner: device,
        })
        .collect())
}

#[derive(Clone, Debug)]
enum DevicePredicate {
    Input,
    Output,
    Any,
}

impl DevicePredicate {
    fn is_satisfied_by(&self, device: &cpal::Device) -> bool {
        match self {
            DevicePredicate::Input => Self::is_input_device(device),
            DevicePredicate::Output => Self::is_output_device(device),
            DevicePredicate::Any => true,
        }
    }

    fn is_input_device(device: &cpal::Device) -> bool {
        device.default_input_config().is_ok() && device.supported_input_configs().is_ok()
    }
    fn is_output_device(device: &cpal::Device) -> bool {
        device.default_output_config().is_ok() && device.supported_output_configs().is_ok()
    }
}

#[derive(Clone)]
pub(crate) struct AudioDevice {
    host_id: cpal::HostId,
    device_index: usize,
    device_name: String,
    inner: cpal::Device,
}

impl std::fmt::Debug for AudioDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioDevice")
            .field("host_id", &self.host_id)
            .field("device_index", &self.device_index)
            .field("device_name", &self.device_name)
            .finish()
    }
}

impl AudioDevice {
    /// Given the name of an audio device that was presumably previously returned by
    /// [`list_devices`], try to find the device now.  A predicate function parameter is used to
    /// ensure the device is suitable for the caller's needs, such as checking for input or output.
    ///
    /// If the device is not found, a meaningful error is returned.
    #[instrument(err, skip(predicate), level = "debug")]
    fn find_device_by_name(name: &str, predicate: DevicePredicate) -> Result<Self> {
        for host_id in cpal::available_hosts() {
            match cpal::host_from_id(host_id) {
                Ok(host) => match host.devices() {
                    Ok(devices) => {
                        for (device_index, device) in devices.enumerate() {
                            if device.name().ok().as_deref() == Some(name)
                                && predicate.is_satisfied_by(&device)
                            {
                                return Ok(AudioDevice {
                                    host_id,
                                    device_index,
                                    device_name: name.to_string(),
                                    inner: device,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        debug!(host_id = host_id.name(), %e, "Error listing devices for host");
                    }
                },
                Err(e) => {
                    debug!(host_id = host_id.name(), %e, "Host appears to be unavailable");
                }
            }
        }

        anyhow::bail!(
            "Audio {}device not found: {name}",
            match predicate {
                DevicePredicate::Input => "input ",
                DevicePredicate::Output => "output ",
                DevicePredicate::Any => "",
            }
        );
    }

    /// Check if this is an input device suitable for use with audio processing models.
    /// If so, convert it into [`AudioInputDevice`], if not return `self` as `Err`.
    fn try_into_input_device(self) -> Result<AudioInputDevice, Self> {
        match (
            self.inner.default_input_config(),
            self.inner.supported_input_configs(),
        ) {
            (Ok(default_config), Ok(supported_input_configs)) => Ok(AudioInputDevice {
                device: self,
                default_config,
                supported_configs: supported_input_configs.collect(),
            }),
            (default_config, supported_configs) => {
                let supported_configs = supported_configs.map(|iter| iter.collect::<Vec<_>>());
                debug!(device_name = %self.device_name,
                    device_index = self.device_index,
                    ?default_config,
                    ?supported_configs,
                    "Device does not support input");
                Err(self)
            }
        }
    }

    /// Check if this is an output device suitable for use with audio processing models.
    /// If so, convert it into [`AudioOutputDevice`], if not return `self` as `Err`.
    fn try_into_output_device(self) -> Result<AudioOutputDevice, Self> {
        match (
            self.inner.default_output_config(),
            self.inner.supported_output_configs(),
        ) {
            (Ok(default_config), Ok(supported_output_configs)) => Ok(AudioOutputDevice {
                device: self,
                default_config,
                supported_configs: supported_output_configs.collect(),
            }),
            (default_config, supported_configs) => {
                let supported_configs = supported_configs.map(|iter| iter.collect::<Vec<_>>());
                debug!(device_name = %self.device_name,
                    device_index = self.device_index,
                    ?default_config,
                    ?supported_configs,
                    "Device does not support output");
                Err(self)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct AudioInputDevice {
    device: AudioDevice,
    default_config: cpal::SupportedStreamConfig,
    supported_configs: Vec<cpal::SupportedStreamConfigRange>,
}

impl AudioInputDevice {
    /// Given the name of an audio input device that was presumably previously returned by
    /// [`list_devices`], try to find the device now and use it as an input device.
    pub fn find_device_by_name(name: &str) -> Result<Self> {
        if let Ok(device) =
            AudioDevice::find_device_by_name(name, DevicePredicate::Input)?.try_into_input_device()
        {
            Ok(device)
        } else {
            anyhow::bail!("Audio device {name} does not appear to be an input device")
        }
    }

    /// Find an audio input device that satisfies the given [`super::AudioSource`] enum.
    ///
    /// If the request cannot be satisfied, fails with a meaningful error.
    pub fn find_device(source: super::AudioSource) -> Result<Self> {
        match source {
            super::AudioSource::Default => {
                let host = cpal::default_host();
                let device = host
                    .default_input_device()
                    .ok_or_else(|| anyhow::anyhow!("No default input device found"))?;
                let device_name = device.name().unwrap_or_default();
                let default_config = device.default_input_config().with_context(|| {
                    format!("No default input config for default input device '{device_name}'")
                })?;
                let supported_configs = device.supported_input_configs()
                    .with_context(|| format!("Supported input configs missing or unable to be retrieved for default input device '{device_name}'"))?
                    .collect();
                Ok(Self {
                    device: AudioDevice {
                        host_id: host.id(),
                        device_index: 0,
                        device_name: device.name().unwrap_or_default(),
                        inner: device,
                    },
                    default_config,
                    supported_configs,
                })
            }
            super::AudioSource::Device(name) => Self::find_device_by_name(&name),
            super::AudioSource::File(_) => {
                anyhow::bail!(
                    "BUG: File device should have been handled at a higher level of abstraction"
                )
            }
        }
    }
}

impl super::AudioInput for AudioInputDevice {
    fn device_name(&self) -> &str {
        &self.device.device_name
    }

    #[instrument(skip_all, fields(device_name = %self.device.device_name))]
    fn start_stream(
        &mut self,
        config: Arc<audio::AudioInputConfig>,
    ) -> Result<(
        Box<dyn super::AudioStreamGuard>,
        Box<dyn super::AudioInputStream>,
    )> {
        debug!(isr = ?config.input_sample_rate, "Selecting stream config for input device");

        // Determine the stream configuration to use based on the audio input config.
        // The heuristic is as follows, and possibly will need to be refined:
        // - If a sample rate is requested, use that.  If that sample rate isn't supported, fail.
        // - If no sample rate is requested explicitly:
        //   - If the default config is suitable (meaning more samples than the standard ASR model
        //   sample rate), use that.
        //   - If the default config is not suitable, find the supported config with a sampling
        //   rate that is closest to the default model sample rate without being lower than it.
        //   - Failing that, use the default config if present (we will have to upsample)
        //   - If there is no default config at all, pick the highest supported sampling rate (we
        //   will have to upsample)
        //
        // XXX: Can we assume that the supported config ranges have already been sorted according
        // to the preferences described at
        // <https://docs.rs/cpal/latest/cpal/struct.SupportedStreamConfigRange.html#method.cmp_default_heuristics>?
        // If so then we can just take the first acceptable sample rate, knowing that W32 format
        // will be prioritized
        // TODO: For now we only support W32 format.  We always need f32 samples for the model, and I
        // don't have code yet that would convert other formats.
        let stream_config = match config.input_sample_rate {
            Some(input_sample_rate) => {
                // Caller has requested a specific sample rate.  If it's not supported, fail.
                debug!(
                    input_sample_rate = input_sample_rate.get(),
                    "Requested specific sample rate"
                );
                if self.default_config.sample_rate().0 == input_sample_rate.get()
                    && self.default_config.sample_format() == cpal::SampleFormat::F32
                {
                    self.default_config.clone()
                } else {
                    self.supported_configs
                        .iter()
                        .filter(|range| range.sample_format() == cpal::SampleFormat::F32)
                        .flat_map(|range| {
                            range.try_with_sample_rate(cpal::SampleRate(input_sample_rate.get()))
                        })
                        .next()
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Requested sample rate {input_sample_rate} not supported by device"
                            )
                        })?
                }
            }
            None => {
                debug!(
                    model_sample_rate = config.model_sample_rate.get(),
                    "Automatically choosing sample rate based on model's required sample rate"
                );
                let default_sample_rate = self.default_config.sample_rate().0;
                if default_sample_rate >= config.model_sample_rate.get() {
                    debug!("Default config is good enough");
                    self.default_config.clone()
                } else {
                    let suitable_configs = self
                        .supported_configs
                        .iter()
                        .filter(|range| {
                            range.max_sample_rate().0 >= config.model_sample_rate.get()
                                && range.sample_format() == cpal::SampleFormat::F32
                        })
                        .sorted_by_key(|range| range.max_sample_rate().0)
                        .inspect(|range| debug!(?range, "Possible config"))
                        .collect::<Vec<_>>();
                    suitable_configs
                        .first()
                        .map(|range| (*(*range)).with_max_sample_rate())
                        .or_else(|| Some(self.default_config.clone()))
                        .ok_or_else(|| anyhow::anyhow!("No suitable sample rate found"))?
                }
            }
        };

        debug!(?stream_config, "Selected stream config");

        let channel_count = stream_config.channels() as usize;

        // If the channel count is more than 1, merge the channels unless an explicit
        // channel has been specified
        // TODO: implement
        if config.channel.is_some() {
            anyhow::bail!(
                "Choosing a specific channel from multi-channel audio is not yet implemented"
            );
        }

        if channel_count > 1 {
            warn!(
                channel_count,
                "In multi-channel audio, only the first channel is used currently"
            );
        }

        let device_name = self.device.device_name.clone();
        let sample_rate = NonZeroU32::new(stream_config.sample_rate().0).unwrap();

        // Create the stream and guard objects
        // The bound of the buffer is calculated based on the max buffered duration divided by the
        // max buffer size.  This is a heuristic that may need to be refined.
        let max_buffered_samples = (config.max_buffered_duration.as_secs_f64()
            / stream_config.sample_rate().0 as f64) as usize;
        let bound = match stream_config.buffer_size() {
            cpal::SupportedBufferSize::Unknown => {
                // Oh boy!  We don't know the buffer size.  Let's just guess.
                warn!("Unknown buffer size; guessing");
                max_buffered_samples / 4096
            }
            cpal::SupportedBufferSize::Range { max, .. } => max_buffered_samples / *max as usize,
        };

        debug!(bound,
            ?config.max_buffered_duration,
            buffer_size = ?stream_config.buffer_size(),
            "Ready to start a worker thread to contain the CPAL stream");

        let (stop_signal_sender, chunks_receiver) =
            cpal_stream_host(stream_config, config.clone(), self.device.clone(), bound)?;

        let stream = stream::AudioInputThreadingChunksReceiver::new(
            device_name,
            sample_rate,
            chunks_receiver,
        );
        Ok((Box::new(Some(stop_signal_sender)), Box::new(stream)))
    }
}

/// Spawn a dedicated thread to host the [`cpal::Stream`], with crossbeam channels to signal when
/// to terminate the stream, and to receive the audio data from the stream.
///
/// That this is necessary at all is a bit unfortunate.  `cpal::Stream` is `!Send` and `!Sync`,
/// apparently because the Android AAudio API isn't thread safe so to ensure that the Rust CPAL API
/// is cross-platform it has to adopt the restrictions of the lowest common denominator.
///
/// Because of this, the `cpal::Stream` has to be created and live it's entire life in one thread.
/// That doesn't work at all with our design, so this host function is the result.
///
/// It spawns a thread and in that thread creates the stream.  It sets up a mechanism to signal
/// that thread when the stream should be dropped (meaning no more streaming audio data), and
/// another channel to transfer the audio data to other threads.
#[instrument(skip_all)]
fn cpal_stream_host(
    stream_config: cpal::SupportedStreamConfig,
    config: Arc<audio::AudioInputConfig>,
    device: AudioDevice,
    chunk_channel_bound: usize,
) -> Result<(
    threading::StopSignalSender,
    threading::CombinedOutputReceiver<AudioInputChunk>,
)> {
    let (stop_signal_sender, running_receiver, final_receiver) =
        threading::start_func_as_thread_worker(
            "cpal_stream_host_thread",
            chunk_channel_bound,
            move |stop_signal_receiver, chunk_sender| {
                let channel_count = stream_config.channels() as usize;
                let mut timestamp = Duration::default();
                let mut samples: u64 = 0;

                let data_callback_sender = chunk_sender.clone();
                let error_callback_sender = chunk_sender;

                let cpal_stream = device.inner.build_input_stream(
                &stream_config.config(),
                move |pcm: &[f32], _: &cpal::InputCallbackInfo| {
                    let pcm = if channel_count == 1 {
                        pcm.to_vec()
                    } else {
                        // For now, just take the first channel and ignore all other channels.
                        pcm.iter()
                            .step_by(channel_count)
                            .copied()
                            .collect::<Vec<f32>>()
                    };
                    if !pcm.is_empty() {
                        let duration = Duration::from_secs_f64(
                            pcm.len() as f64 / stream_config.sample_rate().0 as f64,
                        );
                        let chunk = audio::AudioInputChunk {
                            timestamp,
                            duration,
                            samples: pcm,
                        };
                        timestamp += duration;
                        samples += chunk.samples.len() as u64;

                        // TODO: detect when the channel is full, and record the fact that we drop the samples that could
                        // not be queued.  As written now bits of audio will just drop out but we
                        // should adjust the message payload in the channel to allow it to
                        // represent either a chunk of samples or information about dropped
                        // samples.
                        if let Err(e) = data_callback_sender.try_send(Ok(chunk)) {
                            if let crossbeam::channel::TrySendError::Full(Ok(chunk)) = e {
                                    warn!(samples = chunk.samples.len(), ?chunk.timestamp, ?chunk.duration, "Audio channel is full; dropping all of the samples in this chunk");
                            }
                        }
                    }
                },
                move |err| {
                    // XXX: This will block if the channel is full, until the receiver drops or
                    // until there's spacein the channel.  I'm sure blocking the error callback is
                    // also bad, but in this case something's already broken so I think it's
                    // probably better to block but make sure downstream sees the error, rather
                    // than hiding it.
                    let _ = error_callback_sender.send(Err(err).context("Error in audio stream"));
                },
                None,
            )?;

                debug!("Starting audio stream");
                cpal_stream.play().context("Error starting audio stream")?;

                // Wait for the abort signal.  This might seem a bit odd, but CPAL has already launched its
                // own separate thread that will process the audio from the device and invoke our data
                // callback, and the other end of the channel is held by whoever initiated this stream in
                // the first place, so the only purpose of this thread now is to wait until the stream is
                // stopped, as indicated by a message being sent on this abort receiver or alternatively by
                // abort sender being dropped, at which point we know we can exit this thread and allow the
                // `cpal::Stream` struct to go out of scope and be dropped.
                stop_signal_receiver.wait_for_stop();

                debug!("Audio stream thread exiting");
                Result::<()>::Ok(())
            },
        );

    Ok((
        stop_signal_sender,
        threading::CombinedOutputReceiver::join(running_receiver, final_receiver),
    ))
}

#[derive(Clone, Debug)]
pub(crate) struct AudioOutputDevice {
    device: AudioDevice,
    default_config: cpal::SupportedStreamConfig,
    supported_configs: Vec<cpal::SupportedStreamConfigRange>,
}

impl AudioOutputDevice {
    /// Given the name of an audio output device that was presumably previously returned by
    /// [`list_devices`], try to find the device now and use it as an output device.
    pub(crate) fn find_device_by_name(name: &str) -> Result<Self> {
        if let Ok(device) = AudioDevice::find_device_by_name(name, DevicePredicate::Output)?
            .try_into_output_device()
        {
            Ok(device)
        } else {
            anyhow::bail!("Audio device {name} does not appear to be an output device")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use super::*;
    use assertables::assert_contains;
    use audio::AudioInput;

    #[test]
    fn test_list_device_names() -> Result<()> {
        crate::test_helpers::init_test_logging();

        let (inputs, outputs) = list_device_names()?;

        // Verify that any device names we got are non-empty strings
        for input in &inputs {
            assert!(!input.is_empty(), "Input device name should not be empty");
        }
        for output in &outputs {
            assert!(!output.is_empty(), "Output device name should not be empty");
        }

        Ok(())
    }

    #[test]
    fn test_list_devices() -> Result<()> {
        crate::test_helpers::init_test_logging();

        let (inputs, outputs) = list_devices()?;

        // Test that any found input devices have valid configurations
        for input in &inputs {
            assert!(
                !input.supported_configs.is_empty(),
                "Input device should have at least one supported config"
            );
            assert_valid_stream_config(&input.default_config);
        }

        // Test that any found output devices have valid configurations
        for output in &outputs {
            assert!(
                !output.supported_configs.is_empty(),
                "Output device should have at least one supported config"
            );
            assert_valid_stream_config(&output.default_config);
        }

        Ok(())
    }

    #[test]
    fn test_device_name_lookup() -> Result<()> {
        crate::test_helpers::init_test_logging();

        let (inputs, outputs) = list_device_names()?;

        // Test looking up each input device by name
        for input_name in inputs {
            AudioInputDevice::find_device_by_name(&input_name)
                .expect("Should be able to look up existing input device");
        }

        // Test looking up each output device by name
        for output_name in outputs {
            AudioOutputDevice::find_device_by_name(&output_name)
                .expect("Should be able to look up existing output device");
        }

        Ok(())
    }

    #[test]
    fn test_nonexistent_device_lookup() {
        crate::test_helpers::init_test_logging();

        let result = AudioDevice::find_device_by_name("NonexistentDevice123", DevicePredicate::Any);
        assert!(result.is_err(), "Looking up nonexistent device should fail");
    }

    #[test]
    fn test_invalid_device_type_lookup() -> Result<()> {
        crate::test_helpers::init_test_logging();

        let (inputs, outputs) = list_device_names()?;

        // Try to open input devices as output devices
        for input_name in inputs {
            let result = AudioOutputDevice::find_device_by_name(&input_name);
            if let Err(err) = result {
                assert_contains!(format!("{err:?}"), "Audio output device not found");
            }
        }

        // Try to open output devices as input devices
        for output_name in outputs {
            let result = AudioInputDevice::find_device_by_name(&output_name);
            if let Err(err) = result {
                assert_contains!(format!("{err:?}"), "Audio input device not found");
            }
        }

        Ok(())
    }

    fn assert_valid_stream_config(config: &cpal::SupportedStreamConfig) {
        assert!(config.sample_rate().0 > 0, "Sample rate should be positive");
        assert!(config.channels() > 0, "Channel count should be positive");
    }

    #[test]
    fn test_audio_input_stream() -> Result<()> {
        crate::test_helpers::init_test_logging();

        // Skip test if no default input device
        let host = cpal::default_host();
        let Some(_) = host.default_input_device() else {
            debug!("Skipping test_audio_input_stream: no default input device available");
            return Ok(());
        };

        let mut input_device = AudioInputDevice::find_device(super::super::AudioSource::Default)?;

        let config = Arc::new(audio::AudioInputConfig {
            max_buffered_duration: Duration::from_secs(2),
            ..Default::default()
        });

        let (mut guard, mut stream) = input_device.start_stream(config)?;

        // Collect ~1 second of audio
        let mut samples = Vec::new();
        loop {
            let chunk = match stream.next() {
                Some(result) => result.unwrap(),
                None => break,
            };

            samples.extend(chunk.samples);
            if chunk.timestamp.as_secs_f64() > 1.0 {
                guard.stop_stream();
            }
        }

        // Verify we got some samples
        assert!(!samples.is_empty(), "Should have received audio samples");

        // Check that the audio isn't completely silent
        // We use a very small threshold to account for extremely quiet environments
        let has_non_zero = samples.iter().any(|&s| s.abs() > 0.0001);
        assert!(has_non_zero, "Audio appears to be completely silent");

        Ok(())
    }

    #[test]
    fn test_audio_input_invalid_sample_rate() -> Result<()> {
        crate::test_helpers::init_test_logging();

        // Skip test if no default input device
        let host = cpal::default_host();
        let Some(_) = host.default_input_device() else {
            debug!(
                "Skipping test_audio_input_invalid_sample_rate: no default input device available"
            );
            return Ok(());
        };

        let mut input_device = AudioInputDevice::find_device(super::super::AudioSource::Default)?;

        // Try to request an impossibly high sample rate
        let config = Arc::new(audio::AudioInputConfig {
            input_sample_rate: Some(NonZeroU32::new(10_000_000).unwrap()),
            max_buffered_duration: Duration::from_secs(1),
            ..Default::default()
        });

        let result = input_device.start_stream(config);
        assert!(result.is_err(), "Should fail with invalid sample rate");
        assert_contains!(
            format!("{:?}", result.unwrap_err()),
            "Requested sample rate",
            "Error should mention sample rate issue"
        );

        Ok(())
    }

    #[test]
    fn test_audio_input_multi_channel_selection() -> Result<()> {
        crate::test_helpers::init_test_logging();

        // Skip test if no default input device
        let host = cpal::default_host();
        let Some(_) = host.default_input_device() else {
            debug!("Skipping test_audio_input_multi_channel_selection: no default input device available");
            return Ok(());
        };

        let mut input_device = AudioInputDevice::find_device(super::super::AudioSource::Default)?;

        let config = Arc::new(audio::AudioInputConfig {
            max_buffered_duration: Duration::from_secs(1),
            channel: Some(NonZeroUsize::new(1).unwrap()), // Try to select second channel
            ..Default::default()
        });

        let result = input_device.start_stream(config);
        assert!(
            result.is_err(),
            "Should fail when trying to select specific channel"
        );
        assert_contains!(
            format!("{:?}", result.unwrap_err()),
            "not yet implemented",
            "Error should mention channel selection not implemented"
        );

        Ok(())
    }

    #[test]
    fn test_audio_input_stream_clean_termination() -> Result<()> {
        crate::test_helpers::init_test_logging();

        // Skip test if no default input device
        let host = cpal::default_host();
        let Some(_) = host.default_input_device() else {
            debug!("Skipping test_audio_input_stream_clean_termination: no default input device available");
            return Ok(());
        };

        let mut input_device = AudioInputDevice::find_device(super::super::AudioSource::Default)?;

        let config = Arc::new(audio::AudioInputConfig {
            max_buffered_duration: Duration::from_secs(2),
            ..Default::default()
        });

        let (guard, mut stream) = input_device.start_stream(config)?;

        // Get the first chunk and then drop the guard
        let first_chunk = stream.next().expect("Should get at least one chunk")?;
        assert!(
            !first_chunk.samples.is_empty(),
            "First chunk should contain samples"
        );

        // Drop the guard, which should trigger stream termination
        drop(guard);

        // Read any remaining chunks from the stream
        let mut chunk_count = 1; // Count the first chunk we already got
        for chunk_result in stream {
            // Verify each chunk is Ok (no errors during shutdown)
            let chunk = chunk_result?;
            assert!(
                !chunk.samples.is_empty(),
                "Chunks during shutdown should contain samples"
            );
            chunk_count += 1;
        }

        debug!("Received {chunk_count} chunks before stream ended");
        Ok(())
    }
}
