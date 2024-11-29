//! Audio device management functions.
//!
//! Submodules provide specific implementations of the audio device abstractions.
//!
//! Most production uses will use the [`cpal`] implementation which wraps the Rust `cpal` crate
//! which in turn wraps platform-specific audio APIs for actual audio devices.
//!
//! There is also a virtual device implementation for testing and development that operates on
//! audio files.
mod system;
mod file;

use super::stream::{AudioInputStream, AudioStreamGuard};
use anyhow::Result;
use std::{path::PathBuf, sync::Arc};

pub use system::{list_device_names, AudioInputDevice};

#[derive(Clone, Debug)]
pub enum AudioSource {
    /// Use that system default input device.  Fail if there is no suitable default device.
    Default,

    /// Use a specific audio device specified by name
    Device(String),

    /// Use a virtual audio device that reads audio from a file.
    File(PathBuf),
}

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
    fn start_stream(&mut self, config: Arc<super::AudioInputConfig>) -> Result<(Box<dyn AudioStreamGuard>, Box<dyn AudioInputStream>)>;
}
