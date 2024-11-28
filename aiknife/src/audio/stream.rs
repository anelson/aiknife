//! Audio stream abstractions and associated types.
//!
//! Streams represent either audio coming in for processing, or being produced as output.  To the
//! extent possible they are abstracted away from the underlying audio device.
use anyhow::Result;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::PathBuf;
use std::time::Duration;

pub trait AudioStreamGuard: Send {
    /// Explicitly stop the stream.  This is a no-op if the stream is already stopped.
    /// The stream is also stopped once this guard is dropped.
    fn stop_stream(&mut self);
}

/// A stream of audio from an audio input.
///
/// Note that there are some unfortunate collisions in nomenclature here.  This is a conceptual
/// stream of audio data, in the sense that it streams from an unbounded input (usually a
/// microphone).  But it's not a Rust `Stream`, but rather an `Iterator`.  Up to this point there
/// has not been any reason to make the audio processing part of the solution async.
pub trait AudioInputStream: Send + Iterator<Item = Result<AudioInputChunk>> {
    /// The name of the device this input is connected to.
    fn device_name(&self) -> &str;

    /// The sample rate of this audio stream, in samples/second (aka Hertz).
    ///
    /// This is not related to the sample rate of the source audio device, as part of the audio
    /// processing is resampling the audio to suit the needs of an ASR model.
    fn sample_rate(&self) -> NonZeroU32;
}

/// Implementation of [`AudioInputStream`] that gets the audio samples from a
/// [`crossbeam::channel::bounded::Receiver`], the sending
/// side of which is presumably reading it from a device and pushing it onto the channel.
pub(crate) struct AudioInputCrossbeamChannelReceiver {
    device_name: String,
    sample_rate: NonZeroU32,
    channel: crossbeam::channel::Receiver<Result<AudioInputChunk>>,
}

impl AudioInputCrossbeamChannelReceiver {
    pub fn new(
        device_name: String,
        sample_rate: NonZeroU32,
        channel: crossbeam::channel::Receiver<Result<AudioInputChunk>>,
    ) -> Self {
        Self {
            device_name,
            sample_rate,
            channel,
        }
    }
}

impl AudioInputStream for AudioInputCrossbeamChannelReceiver {
    fn device_name(&self) -> &str {
        &self.device_name
    }

    fn sample_rate(&self) -> NonZeroU32 {
        self.sample_rate
    }
}

impl Iterator for AudioInputCrossbeamChannelReceiver {
    type Item = Result<AudioInputChunk>;

    fn next(&mut self) -> Option<Self::Item> {
        // `recv` is fallible only if the senders are all dropped, meaning there will be no further
        // chunks forthcoming
        self.channel.recv().ok()
    }
}

/// A chunk of PCM float32 audio data from an audio stream.
#[derive(Clone, Debug)]
pub struct AudioInputChunk {
    /// Timestamp of this chunk, relative to the start of the stream.
    pub timestamp: Duration,

    /// The duration of the audio in this chunk.
    pub duration: Duration,
    pub samples: Vec<f32>,
}
