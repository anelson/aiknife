//! System audio device management functions, including convenient functions for discovering all audio
//! devices in a way that's suitable for listing them to users, and picking a reasonable default.
//!
//! TODO: Handle changes to available system audio devices on the fly (ie, AirPods connecting to a
//! Mac).
use crate::audio::{self as audio, stream, AudioInputChunk};
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
pub(crate) struct AudioInputDevice {
    device: AudioDevice,
    default_config: cpal::SupportedStreamConfig,
    supported_configs: Vec<cpal::SupportedStreamConfigRange>,
}

impl AudioInputDevice {
    /// Given the name of an audio input device that was presumably previously returned by
    /// [`list_devices`], try to find the device now and use it as an input device.
    pub(crate) fn find_device_by_name(name: &str) -> Result<Self> {
        if let Ok(device) =
            AudioDevice::find_device_by_name(name, DevicePredicate::Input)?.try_into_input_device()
        {
            Ok(device)
        } else {
            anyhow::bail!("Audio device {name} does not appear to be an input device")
        }
    }

    /// Find an audio input device that satisfies the given [`AudioInputDevice`] enum.
    ///
    /// If the request cannot be satisfied, fails with a meaningful error.
    pub(crate) fn find_device(device: super::AudioInputDevice) -> Result<Self> {
        match device {
            super::AudioInputDevice::Default => {
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
            super::AudioInputDevice::Device(name) => Self::find_device_by_name(&name),
            super::AudioInputDevice::File(_) => {
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
                        .map(|range| {
                            cpal::SupportedStreamConfig::from(
                                (*range).clone().with_max_sample_rate(),
                            )
                        })
                        .or_else(|| Some(self.default_config.clone()))
                        .ok_or_else(|| anyhow::anyhow!("No suitable sample rate found"))?
                }
            }
        };

        debug!(?stream_config, "Selected stream config");

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

        debug!(bound, ?config.max_buffered_duration, buffer_size = ?stream_config.buffer_size(), "Creating crossbeam channel");
        let (sender, receiver) = crossbeam::channel::bounded(bound);

        let guard = cpal_stream_host(stream_config, config.clone(), self.device.clone(), sender);

        let stream =
            stream::AudioInputCrossbeamChannelReceiver::new(device_name, sample_rate, receiver);
        Ok((Box::new(guard), Box::new(stream)))
    }
}

/// See [`cpal_stream_host`] for details.
#[derive(Clone, Debug)]
struct CpalStreamGuard(Arc<crossbeam::channel::Sender<()>>);

impl Drop for CpalStreamGuard {
    fn drop(&mut self) {
        <Self as super::AudioStreamGuard>::stop_stream(self);
    }
}

impl super::AudioStreamGuard for CpalStreamGuard {
    fn stop_stream(&mut self) {
        let _ = self.0.try_send(());
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
    chunk_sender: crossbeam::channel::Sender<Result<AudioInputChunk>>,
) -> CpalStreamGuard {
    /// As if this wasn't mind-bending enough, a nested function that is called from the spawned
    /// cpal::Stream thread, to actually create the stream.  This is a convenience, which let's use
    /// use `?` sigils for error handling, so that the actual thread host only has to check for and
    /// report errors in one place, when it calls this function.
    #[instrument(skip_all)]
    fn create_cpal_stream(
        stream_config: cpal::SupportedStreamConfig,
        config: Arc<audio::AudioInputConfig>,
        device: AudioDevice,
        chunk_sender: crossbeam::channel::Sender<Result<AudioInputChunk>>,
    ) -> Result<cpal::Stream> {
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

        let mut timestamp = Duration::default();
        let mut samples: u64 = 0;

        let data_callback_sender = chunk_sender.clone();
        let error_callback_sender = chunk_sender.clone();

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
                            match e {
                                crossbeam::channel::TrySendError::Full(Ok(chunk)) => {
                                    warn!(samples = chunk.samples.len(), ?chunk.timestamp, ?chunk.duration, "Audio channel is full; dropping all of the samples in this chunk");
                                }
                                crossbeam::channel::TrySendError::Full(Err(_)) => {
                                    unreachable!();
                                }
                                crossbeam::channel::TrySendError::Disconnected(_) => {
                                    debug!("Failed to send audio chunk; receiver has been dropped");
                                }
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
                    let _ = error_callback_sender.send(Err(err).with_context(|| format!("Error in audio stream")));
                },
                None,
            )?;

        Ok(cpal_stream)
    }

    // Use a zero-length channel to signal the stream thread to stop.
    let (abort_sender, abort_receiver) = crossbeam::channel::bounded(0);

    let current_span = Span::current();

    // Create the cpal stream for this device, using callbacks to push the audio data into the
    // crossbeam channel.
    //
    // NOTE: this callback is performance critical, as any latency here is going to be imparted
    // into the audio sampling.  So we just do some very minimal processing here, stripping out
    // any extraneous channels, and copy the data into a new vec which we then send to
    // crossbeam.  All of the logic of buffering and whatever other post-processing will have
    // to be done on the receive side.
    let _thread = std::thread::spawn(move || {
        // Propagate the spawn from the caller into this thread too
        let _guard = current_span.enter();

        debug!("Starting audio stream thread");

        let cpal_stream =
            match create_cpal_stream(stream_config, config, device, chunk_sender.clone()) {
                Ok(stream) => stream,
                Err(e) => {
                    error!(%e, "Error creating audio stream");
                    let _ = chunk_sender
                        .send(Err(e).with_context(|| "Error creating audio stream".to_string()));
                    anyhow::bail!("Error creating audio stream");
                }
            };

        debug!("Starting audio stream");
        if let Err(e) = cpal_stream.play() {
            error!(%e, "Error starting audio stream");
            let _ = chunk_sender.send(Err(e).with_context(|| "Error in audio stream".to_string()));

            // No one will see this returned error from the thread, but when this scope ends the
            // channel sender for audio chunks will go out of scope, which will cause the receiver
            // to fail when it tries to read from an empty channel.
            anyhow::bail!("Error starting audio stream");
        }

        // Wait for the abort signal
        match abort_receiver.recv() {
            Ok(()) => {
                debug!("Received abort signal; stopping audio stream");

                // This doesn't even need to be here; we'll be dropping the whole stream soon.
                cpal_stream.pause().ok();
            }
            Err(e) => {
                debug!(%e, "Error receiving abort signal; assuming that the stream guard struct was dropped and that this constitutes an abort signal");
            }
        }

        debug!("Audio stream thread exiting");
        Result::<()>::Ok(())
    });

    return CpalStreamGuard(Arc::new(abort_sender));
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
    use super::*;
    use assertables::assert_contains;

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
                assert_contains!(err.to_string(), "Audio output device not found");
            }
        }

        // Try to open output devices as input devices
        for output_name in outputs {
            let result = AudioInputDevice::find_device_by_name(&output_name);
            if let Err(err) = result {
                assert_contains!(err.to_string(), "Audio input device not found");
            }
        }

        Ok(())
    }

    fn assert_valid_stream_config(config: &cpal::SupportedStreamConfig) {
        assert!(config.sample_rate().0 > 0, "Sample rate should be positive");
        assert!(config.channels() > 0, "Channel count should be positive");
    }
}
