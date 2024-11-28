//! Audio device management functions, including convenient functions for discovering all audio
//! devices in a way that's suitable for listing them to users, and picking a reasonable default.
//!
//! TODO: Handle changes to available system audio devices on the fly (ie, AirPods connecting to a
//! Mac).
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};
use either::Either;
use itertools::Itertools;
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
    devices.flat_map(|d| d.inner.name().ok()).collect()
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
                    debug!(device_name = %d.inner.name().unwrap_or_default(),
                        device_index = d.device_index,
                        "Device does not support input or output");
                    None
                }
            },
        })
        .inspect(|either| match either {
            Either::Left(d) => {
                debug!(device_name = %d.device.inner.name().unwrap_or_default(),
                        device_index = d.device.device_index,
                        "Found input device")
            }
            Either::Right(d) => {
                debug!(device_name = %d.device.inner.name().unwrap_or_default(),
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
    inner: cpal::Device,
}

impl std::fmt::Debug for AudioDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioDevice")
            .field("host_id", &self.host_id)
            .field("device_index", &self.device_index)
            .field("device", &self.inner.name().unwrap_or_default())
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
                            if device.name().ok().as_deref() == Some(name) {
                                if predicate.is_satisfied_by(&device) {
                                    return Ok(AudioDevice {
                                        host_id,
                                        device_index,
                                        inner: device,
                                    });
                                }
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
                debug!(device_name = %self.inner.name().unwrap_or_default(),
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
                debug!(device_name = %self.inner.name().unwrap_or_default(),
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
