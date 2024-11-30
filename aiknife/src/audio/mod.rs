//! Implementation of audio-related AI functionality (presently Whisper)

mod device;
pub use device::*;
mod stream;
pub use stream::*;

use std::num::{NonZeroU32, NonZeroUsize};
use std::path::PathBuf;
use std::time::Duration;

/// The sample rate that ASR models like Whisper want.  No audio device is likely to support this
/// natively so resampling will be required, however we bias towards a device sample rate that is
/// at least this high to ensure the model has high enough quality input.
const DEFAULT_MODEL_SAMPLE_RATE: u32 = 16000;

/// Possible places we can get audio from
#[derive(Clone, Debug)]
pub enum AudioSource {
    /// Use that system default input device.  Fail if there is no suitable default device.
    Default,

    /// Use a specific audio device specified by name
    Device(String),

    /// Use a virtual audio device that reads audio from a file.
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct AudioInputConfig {
    /// The source of the audio to operate on.
    pub source: AudioSource,

    /// The audio channel (1-based) to use for audio acquisition.  If `None`,
    /// then if the audio device has multiple audio channels they will be merged into a single mono
    /// signal.
    ///
    /// Most normal mic inputs only have a single channel so this is not an issue.  More
    /// sophisticated audio gear might expose multiple channels, some of which might not even be
    /// hooked up to active mics, at which point this becomes an important parameter.
    pub channel: Option<NonZeroUsize>,

    /// How much audio to buffer initially before yielding it to the consumer of the stream.
    pub initial_buffer_duration: Duration,

    /// After the initial buffer duration, how much audio to acquire from the device before
    /// yielding it to the consumer of the stream.
    pub buffer_duration: Duration,

    /// The maximum amount of audio to buffer for the consumer of the stream to process.  In most
    /// cases the consumer can consume the audio faster than the audio itself is produced so this
    /// parameter is not needed.  However on slow systems with heavy ASR models the processing
    /// pipeline may fall behind.  In that case, the audio will be buffered up to this duration,
    /// after which audio samples will be dropped and errors reported in the log.
    pub max_buffered_duration: Duration,

    /// The sample rate (in samples/sec aka Hertz) to use for audio acquisition.  If `None`, then
    /// the default sample rate of the audio device will be used, or if that is not suitable to the
    /// ASR model then some other reasonable default.
    ///
    /// Note that in most cases the ASR model will require us to resample the audio anyway.  It's
    /// not clear what the use case is for this parameter, but it's included for completeness.
    ///
    /// Note also that when the input device is a WAV file, this really should not be specified.
    /// If it is, and it doesn't match the sample rate in the WAV file, an error ocurrs.
    pub input_sample_rate: Option<NonZeroU32>,

    /// The sample rate that the model that will be consuming this audio requires.
    pub model_sample_rate: NonZeroU32,
}

impl Default for AudioInputConfig {
    fn default() -> Self {
        Self {
            source: AudioSource::Default,
            channel: None,
            initial_buffer_duration: Duration::from_secs(1),
            buffer_duration: Duration::from_millis(200),
            max_buffered_duration: Duration::from_secs(30),
            input_sample_rate: None,
            model_sample_rate: NonZeroU32::new(DEFAULT_MODEL_SAMPLE_RATE).unwrap(),
        }
    }
}

// TODO:
// - Streaming audio from a device or file
// - List all audio devices
// - Code to support streaming transcription (only from a live device unless we're testing with an
// audio file)
//   - Initially take a minimum of one second of audio and pass it to Whisper
//   - Do the Whisper inference in a separate task so that we can keep buffering audio
//   - Apply back-pressure so if inference takes more than one second we keep buffering and run
//   inference again as soon as we can
//   - Keep track of the output tokens; if outputs are repeated in N inference steps (default is 2)
//   then they are confirmed, otherwise they are just potential tokens
//   - Figure out where to step the audio and start again.  This will involve using VAD, but it's
//   not as simple as chopping a segment whenever the VAD indicates lack of voice.  Until the user
//   is done talking, we want to keep processing the audio up to the last 30 seconds.  Once we have
//   more than 30 seconds of audio, then we need to truncate and for that we'll use VAD.
//   - Later I want to add some filters like DeepNetFilter and possibly something to normalize
//   volume
//   - Disable Whisper timestamps they are useless
//   - Feed previous text back into the context
//   - Run VAD on the input as a way to detect when thre is no new voice since we last ran
//   inference; that's a performance/battery life optimization.  When the user starts to talk
//   again, we'll still feed in up to the last 30 seconds of audio, but only actual voice; long
//   pauses should be filtered out.
//
