//! Audio device management functions.
//!
//! Submodules provide specific implementations of the audio device abstractions.
//!
//! Most production uses will use the [`system`] implementation which wraps the Rust `cpal` crate
//! which in turn wraps platform-specific audio APIs for actual audio devices.
//!
//! There is also a virtual device implementation for testing and development that operates on
//! audio files.
mod file;
mod system;

use super::stream::{AudioInputStream, AudioStreamGuard};
use anyhow::Result;
use std::sync::Arc;

pub use system::list_device_names;

pub trait AudioInput: Send {
    /// The name of the device this input is connected to.
    fn device_name(&self) -> &str;

    /// Start streaming audio from the device.
    ///
    /// Fails if the device is already streaming.
    ///
    /// Returns a guard object that must be kept alive for the duration of the stream.  When the
    /// guard is dropped, the stream will stop.
    ///
    /// Also returns the stream object itself which can be used to interact with the stream.
    fn start_stream(
        &mut self,
        config: super::AudioInputConfig,
    ) -> Result<(Box<dyn AudioStreamGuard>, Box<dyn AudioInputStream>)>;
}

pub fn open_audio_input(source: super::AudioSource) -> Result<Box<dyn AudioInput>> {
    match source {
        super::AudioSource::Default | super::AudioSource::Device(_) => {
            Ok(Box::new(system::DeviceAudioInput::from_source(source)?))
        }
        super::AudioSource::File(path) => Ok(Box::new(file::FileAudioInput::new(path)?)),
    }
}
